//! Use Codex's prompt history for terminal draft recall, separate from model input.

use super::ThreadRequestProcessor;
use crate::error_code::internal_error;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ThreadInputHistoryAppendParams;
use codex_app_server_protocol::ThreadInputHistoryAppendResponse;
use codex_app_server_protocol::ThreadInputHistoryEntry;
use codex_app_server_protocol::ThreadInputHistoryReadParams;
use codex_app_server_protocol::ThreadInputHistoryReadResponse;
use codex_message_history::HistoryConfig;
use codex_message_history::append_entry;
use codex_message_history::recent_entries;

impl ThreadRequestProcessor {
    #[tracing::instrument(skip_all)]
    pub(crate) async fn thread_input_history_read(
        &self,
        params: ThreadInputHistoryReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let (_, thread) = self.load_thread(&params.thread_id).await?;
        let config = thread.config().await;
        let history = HistoryConfig::new(config.codex_home.to_path_buf(), &config.history);
        let entries = tokio::task::spawn_blocking(move || recent_entries(&history))
            .await
            .map_err(|err| internal_error(format!("failed to read terminal input history: {err}")))?
            .map_err(|err| {
                internal_error(format!("failed to read terminal input history: {err}"))
            })?;
        let data = entries
            .into_iter()
            .map(|entry| ThreadInputHistoryEntry {
                session_id: entry.session_id,
                text: entry.text,
            })
            .collect();
        Ok(Some(ThreadInputHistoryReadResponse { data }.into()))
    }

    #[tracing::instrument(skip_all)]
    pub(crate) async fn thread_input_history_append(
        &self,
        params: ThreadInputHistoryAppendParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let (id, thread) = self.load_thread(&params.thread_id).await?;
        if !params.text.trim().is_empty() {
            let config = thread.config().await;
            let history = HistoryConfig::new(config.codex_home.to_path_buf(), &config.history);
            append_entry(&params.text, id, &history)
                .await
                .map_err(|err| {
                    internal_error(format!("failed to save terminal input history: {err}"))
                })?;
        }
        Ok(Some(ThreadInputHistoryAppendResponse {}.into()))
    }
}
