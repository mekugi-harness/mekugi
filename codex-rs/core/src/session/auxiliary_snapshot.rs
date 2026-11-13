//! Borrow history at its publication boundary, without pausing primary execution.

use super::Session;
use crate::context_manager::ContextManager;
use codex_history::ResponseItemEnvelope;
use codex_protocol::error::{CodexErr, Result};
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AgentStatus;
use std::collections::HashSet;
use std::sync::Mutex;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct PendingSnapshots(Mutex<Vec<oneshot::Sender<ContextManager>>>);

impl Session {
    pub(crate) async fn progress_history_snapshot(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<ContextManager> {
        if cancellation.is_cancelled() {
            return Err(CodexErr::TurnAborted);
        }
        let mut status = self.agent_status.subscribe();
        let snapshot = {
            // Registration and publication use the history lock, so a completed
            // output cannot slip between the check and the subscription.
            let state = self.state.lock().await;
            let history = state.history.annotated_items();
            if !self.services.code_mode_service.has_active_cells()
                && completed_tool_items(history).count() == history.len()
            {
                return Ok(state.clone_history());
            }
            let requests = self.services.thread_extension_data
                .get_or_init(PendingSnapshots::default);
            let mut requests = requests.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            requests.retain(|request| !request.is_closed());
            let (send, receive) = oneshot::channel();
            requests.push(send);
            receive
        };
        tokio::pin!(snapshot);
        loop {
            let current = status.borrow_and_update().clone();
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CodexErr::TurnAborted),
                snapshot = &mut snapshot => return snapshot.map_err(|_| CodexErr::InternalAgentDied),
                _ = std::future::ready(()), if !matches!(current, AgentStatus::Running | AgentStatus::PendingInit) => {
                    return Err(CodexErr::InvalidRequest(format!("progress context unavailable: {current:?}")));
                }
                changed = status.changed() => {
                    if changed.is_err() {
                        return Err(CodexErr::InternalAgentDied);
                    }
                }
            }
        }
    }

    /// Called under the history lock only after an actual tool output is stored.
    /// Yielded process/cell handles remain verbatim; no process exit is required.
    pub(super) fn publish_progress_snapshots(&self, history: &ContextManager) {
        let Some(requests) = self.services.thread_extension_data.get::<PendingSnapshots>() else {
            return;
        };
        for request in requests.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain(..) {
            if !request.is_closed() {
                let _ = request.send(history.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use std::task::Poll;

    fn item(value: serde_json::Value) -> ResponseItem {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn snapshot_waits_for_published_output_and_retains_running_handles() {
        let (session, context, _events) = crate::session::tests::make_session_and_context_with_rx().await;
        session.agent_status.send_replace(AgentStatus::Running);
        session.record_conversation_items(&context, context.model_info(), &[
            item(serde_json::json!({"type":"custom_tool_call","call_id":"call","name":"exec","input":"run"})),
        ]).await;
        let cancellation = CancellationToken::new();
        let mut snapshot = Box::pin(session.progress_history_snapshot(&cancellation));
        assert!(matches!(futures::poll!(snapshot.as_mut()), Poll::Pending));
        // Non-output history does not stand in for a completed tool boundary.
        session.record_conversation_items(&context, context.model_info(), &[
            item(serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"still working"}]})),
        ]).await;
        assert!(matches!(futures::poll!(snapshot.as_mut()), Poll::Pending));
        let output = item(serde_json::json!({"type":"custom_tool_call_output","call_id":"call","output":"session_id=42; next_call: mread retained"}));
        session.record_conversation_items(&context, context.model_info(), &[output.clone()]).await;
        let history = snapshot.now_or_never().expect("publication resolves the snapshot").unwrap();
        assert!(history.raw_items().any(|item| matches!(item,
            ResponseItem::CustomToolCallOutput { call_id, output, .. }
                if call_id == "call" && output == &codex_protocol::models::FunctionCallOutputPayload::from_text("session_id=42; next_call: mread retained".to_string())
        )));
        assert_eq!(completed_tool_items(history.annotated_items()).count(), history.annotated_items().len());
    }

    #[tokio::test]
    async fn yielded_cell_requires_a_new_output_not_cell_termination() {
        let (session, context, _events) = crate::session::tests::make_session_and_context_with_rx().await;
        session.agent_status.send_replace(AgentStatus::Running);
        let cell = codex_code_mode::CellId::new("background".to_owned());
        session.services.code_mode_service.mark_cell_ready_for_dispatch(&cell, None);
        let cancellation = CancellationToken::new();
        let mut snapshot = Box::pin(session.progress_history_snapshot(&cancellation));
        assert!(matches!(futures::poll!(snapshot.as_mut()), Poll::Pending));
        let output = item(serde_json::json!({"type":"custom_tool_call_output","call_id":"wait","output":"Script running with cell ID background"}));
        session.record_conversation_items(&context, context.model_info(), &[output.clone()]).await;
        let history = snapshot.now_or_never().unwrap().unwrap();
        assert!(history.raw_items().any(|item| matches!(item,
            ResponseItem::CustomToolCallOutput { call_id, output, .. }
                if call_id == "wait" && output == &codex_protocol::models::FunctionCallOutputPayload::from_text("Script running with cell ID background".to_string())
        )));
        assert!(session.services.code_mode_service.has_active_cells());
        session.services.code_mode_service.finish_cell_dispatch(&cell);
    }

    #[tokio::test]
    async fn cancellation_and_target_failure_settle_only_the_snapshot() {
        let (session, _context, _events) = crate::session::tests::make_session_and_context_with_rx().await;
        let cell = codex_code_mode::CellId::new("background".to_owned());
        session.services.code_mode_service.mark_cell_ready_for_dispatch(&cell, None);
        session.agent_status.send_replace(AgentStatus::Running);
        let cancellation = CancellationToken::new();
        let mut snapshot = Box::pin(session.progress_history_snapshot(&cancellation));
        assert!(matches!(futures::poll!(snapshot.as_mut()), Poll::Pending));
        cancellation.cancel();
        assert!(snapshot.await.is_err());
        assert!(session.services.code_mode_service.has_active_cells());
        assert!(!session.services.local_agent_runtime.shutdown.is_cancelled());
        let cancellation = CancellationToken::new();
        let mut snapshot = Box::pin(session.progress_history_snapshot(&cancellation));
        assert!(matches!(futures::poll!(snapshot.as_mut()), Poll::Pending));
        session.agent_status.send_replace(AgentStatus::Errored("provider stopped".into()));
        let error = snapshot.await.unwrap_err().to_string();
        assert!(error.contains("provider stopped"));
        session.services.code_mode_service.finish_cell_dispatch(&cell);
    }
}

pub(crate) fn completed_tool_items(
    history: &[ResponseItemEnvelope],
) -> impl Iterator<Item = &ResponseItemEnvelope> {
    let mut function_outputs = HashSet::new();
    let mut custom_outputs = HashSet::new();
    let mut search_outputs = HashSet::new();
    for envelope in history {
        match &envelope.item {
            ResponseItem::FunctionCallOutput { call_id: Some(id), .. } => {
                function_outputs.insert(id);
            }
            ResponseItem::CustomToolCallOutput { call_id, .. } => {
                custom_outputs.insert(call_id);
            }
            ResponseItem::ToolSearchOutput { call_id: Some(id), .. } => {
                search_outputs.insert(id);
            }
            _ => {}
        }
    }
    history.iter().filter(move |envelope| match &envelope.item {
        ResponseItem::FunctionCall { call_id, .. }
        | ResponseItem::LocalShellCall { call_id: Some(call_id), .. } => function_outputs.contains(call_id),
        ResponseItem::CustomToolCall { call_id, .. } => custom_outputs.contains(call_id),
        ResponseItem::ToolSearchCall { call_id: Some(call_id), .. } => search_outputs.contains(call_id),
        _ => true,
    })
}
