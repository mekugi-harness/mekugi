use super::THREAD_ITEMS_MAX_LIMIT;
use super::ThreadRequestProcessor;
use super::ensure_direct_input_allowed;
use super::invalid_request;
use super::stored_thread_item_to_api_item;
use super::thread_store_mutation_error;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ThreadItemPayload;
use codex_app_server_protocol::ThreadRevertPreviewParams;
use codex_app_server_protocol::ThreadRevertPreviewResponse;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeId;
use codex_change_history::ChangeRecord;
use codex_change_history::MutationDirection;
use codex_core::CodexThread;
use codex_protocol::ThreadId;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_thread_store::ItemSortKey;
use codex_thread_store::ListItemsParams;
use codex_thread_store::RevertThreadParams;
use codex_thread_store::SortDirection;
use codex_thread_store::StoredThreadItem;
use std::collections::HashMap;
use std::collections::HashSet;

pub(super) struct RewindScope {
    root: ThreadId,
    members: HashSet<String>,
}

struct RewindEdits {
    store: ChangeHistoryStore,
    session_id: String,
    records: Vec<ChangeRecord>,
}

impl RewindEdits {
    fn ids(&self) -> impl Iterator<Item = &ChangeId> {
        self.records.iter().map(|record| &record.id)
    }

    async fn mutate(
        store: ChangeHistoryStore,
        session_id: String,
        ids: Vec<ChangeId>,
    ) -> Result<(ChangeHistoryStore, ChangeId, String), JSONRPCErrorError> {
        tokio::task::spawn_blocking(move || {
            let outcome = store
                .mutate_workspace(
                    MutationDirection::Revert,
                    &ids,
                    /*path_filters*/ &[],
                    // The person chose this operation; it is not part of any model execution.
                    /*actor_thread_id*/
                    "",
                    &session_id,
                )
                .map_err(change_error)?;
            Ok((store, outcome.recorded.id, session_id))
        })
        .await
        .map_err(change_error)?
    }
}

impl ThreadRequestProcessor {
    pub(crate) async fn thread_revert_preview(
        &self,
        params: ThreadRevertPreviewParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let (id, thread) = self.load_thread(&params.thread_id).await?;
        ensure_direct_input_allowed(thread.as_ref()).await?;
        let planned = async {
            let config = thread.config_snapshot().await;
            if config.ephemeral || config.history_mode != ThreadHistoryMode::Paginated {
                return Err(invalid_request(
                    "File undo requires saved paginated history",
                ));
            }
            let scope = self.idle_rewind_scope(id, thread.as_ref()).await?;
            self.rewind_edits(
                scope,
                id,
                &params.before_turn_id,
                params.before_item_id.as_deref(),
            )
            .await
        }
        .await;
        let validated = match planned {
            Ok(plan) => tokio::task::spawn_blocking(move || {
                plan.store
                    .validate_workspace_mutation(MutationDirection::Revert, &plan.records)
                    .map_err(change_error)?;
                Ok(plan.ids().map(ToString::to_string).collect())
            })
            .await
            .map_err(change_error)?,
            Err(error) => Err(error),
        };
        let response = match validated {
            Ok(change_ids) => ThreadRevertPreviewResponse {
                change_ids,
                reason: None,
            },
            Err(error) => ThreadRevertPreviewResponse {
                change_ids: Vec::new(),
                reason: Some(error.message),
            },
        };
        Ok(Some(ClientResponsePayload::ThreadRevertPreview(response)))
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub(super) async fn commit_thread_revert(
        &self,
        params: RevertThreadParams,
        undo_scope: Option<(RewindScope, Vec<String>)>,
    ) -> Result<(), JSONRPCErrorError> {
        let undo = if let Some((scope, expected)) = undo_scope {
            let plan = self
                .rewind_edits(
                    scope,
                    params.thread_id,
                    &params.before_turn_id,
                    params.before_item_id.as_deref(),
                )
                .await?;
            if !plan
                .ids()
                .map(ChangeId::as_str)
                .eq(expected.iter().map(String::as_str))
            {
                return Err(invalid_request(
                    "Captured edits changed; reopen Rewind to review them",
                ));
            }
            let ids = plan.records.into_iter().map(|record| record.id).collect();
            Some(RewindEdits::mutate(plan.store, plan.session_id, ids).await?)
        } else {
            None
        };
        if let Err(error) = self.thread_store.revert_thread(params).await {
            let error = thread_store_mutation_error("revert", error);
            if let Some((store, undo_id, session_id)) = undo
                && let Err(rollback) = RewindEdits::mutate(store, session_id, vec![undo_id]).await
            {
                return Err(super::internal_error(format!(
                    "{}; could not restore the workspace: {}",
                    error.message, rollback.message
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) async fn idle_rewind_scope(
        &self,
        id: ThreadId,
        thread: &CodexThread,
    ) -> Result<RewindScope, JSONRPCErrorError> {
        let family = self.state_db_spawn_subtree_thread_ids(id).await?;
        for member in &family {
            if let Ok(loaded) = self.thread_manager.get_thread(*member).await
                && (loaded.has_active_turn().await
                    || !loaded.list_background_terminals().await.is_empty())
            {
                return Err(invalid_request(
                    "Wait for the thread and its delegates to be idle",
                ));
            }
        }
        Ok(RewindScope {
            root: thread.agent_root_id(),
            members: family.into_iter().map(|id| id.to_string()).collect(),
        })
    }

    async fn rewind_edits(
        &self,
        scope: RewindScope,
        id: ThreadId,
        turn: &str,
        item: Option<&str>,
    ) -> Result<RewindEdits, JSONRPCErrorError> {
        let boundary = self.rewind_boundary_ms(id, turn, item).await?;
        let RewindScope { root, members } = scope;
        let dir =
            codex_protocol::shell_environment::session_tmp_path(root.into()).join("change-history");
        let (store, records) = tokio::task::spawn_blocking(move || {
            if !dir.is_dir() {
                return Err(invalid_request("No captured edits to revert"));
            }
            let store = ChangeHistoryStore::open_existing(&dir).map_err(change_error)?;
            let entries = store.list_entries().map_err(change_error)?;
            let ids: Vec<_> = entries
                .into_iter()
                .filter(|entry| {
                    entry.source_kind.is_authored()
                        && members.contains(&entry.thread_id)
                        && !entry.ephemeral_only
                        && entry.created_at.saturating_add(1).saturating_mul(1000) > boundary
                })
                .map(|entry| entry.id)
                .collect();
            if ids.is_empty() {
                return Err(invalid_request("No captured edits to revert"));
            }
            let records = store.load_records(&ids).map_err(change_error)?;
            Ok((store, records))
        })
        .await
        .map_err(change_error)??;
        let session_id = root.to_string();
        let evidence_by_turn = self.rewind_history_items(&records).await?;
        let mut selected_indices = Vec::new();
        for (index, record) in records.iter().enumerate() {
            let metadata = &record.metadata;
            let evidence = evidence_by_turn
                .get(&(metadata.thread_id.as_str(), metadata.turn_id.as_str()))
                .and_then(|items| items.get(&metadata.call_id));
            let Some(evidence) = evidence else {
                // An earlier Keep can remove the execution but preserve its capture. The
                // capture's second-resolution index time can overlap the new message.
                if record.recorded_at_ms.is_some_and(|time| time < boundary) {
                    continue;
                }
                return Err(invalid_request(
                    "A captured edit has no matching retained execution",
                ));
            };
            let started = evidence.started_at_ms.unwrap_or(evidence.created_at_ms);
            if started <= 0 {
                return Err(invalid_request("Captured edit has no recorded start time"));
            }
            if started < boundary {
                if evidence.completed_at_ms.is_some_and(|end| end < boundary) {
                    continue;
                }
                return Err(invalid_request(
                    "A captured edit overlaps the selected message",
                ));
            }
            if record.files.iter().any(|file| file.scope.is_ephemeral()) {
                return Err(invalid_request(
                    "Captured workspace and temporary edits cannot be separated for this message",
                ));
            }
            selected_indices.push(index);
        }
        if selected_indices.is_empty() {
            return Err(invalid_request("No captured edits after this message"));
        }
        let mut selected_indices = selected_indices.into_iter().peekable();
        let records = records
            .into_iter()
            .enumerate()
            .filter_map(|(index, record)| selected_indices.next_if_eq(&index).map(|_| record))
            .collect();
        Ok(RewindEdits {
            store,
            session_id,
            records,
        })
    }

    async fn rewind_boundary_ms(
        &self,
        thread_id: ThreadId,
        turn_id: &str,
        item_id: Option<&str>,
    ) -> Result<i64, JSONRPCErrorError> {
        let mut position = None;
        loop {
            let page = self
                .thread_store
                .list_items(ListItemsParams {
                    item_ids: item_id.map(|id| vec![id.to_string()]),
                    thread_id,
                    turn_id: Some(turn_id.to_string()),
                    include_archived: false,
                    position: position.clone(),
                    page_size: if item_id.is_some() { 1 } else { 20 },
                    sort_direction: SortDirection::Asc,
                    sort_key: ItemSortKey::CreatedAtOrdinal,
                    after_updated_at_ordinal: None,
                    before_updated_at_ordinal: None,
                })
                .await
                .map_err(change_error)?;
            for item in page.items {
                if matches!(
                    stored_thread_item_to_api_item(&item)?.payload,
                    ThreadItemPayload::UserMessage { .. }
                ) {
                    let time = item.created_at_ms;
                    return (time > 0).then_some(time).ok_or_else(|| {
                        invalid_request("The selected message has no recorded time")
                    });
                }
            }
            let Some(next) = page.next_cursor else {
                return Err(invalid_request(
                    "The selected user message is no longer in the history",
                ));
            };
            position = Some(codex_thread_store::ListItemsPosition::Cursor(next));
        }
    }

    async fn rewind_history_items<'a>(
        &self,
        records: &'a [ChangeRecord],
    ) -> Result<HashMap<(&'a str, &'a str), HashMap<String, StoredThreadItem>>, JSONRPCErrorError>
    {
        let mut grouped = HashMap::<_, HashSet<_>>::new();
        for record in records {
            let metadata = &record.metadata;
            if metadata.turn_id.is_empty() || metadata.call_id.is_empty() {
                continue;
            }
            grouped
                .entry((metadata.thread_id.as_str(), metadata.turn_id.as_str()))
                .or_default()
                .insert(metadata.call_id.as_str());
        }
        let mut evidence = HashMap::with_capacity(grouped.len());
        for ((owner, turn_id), ids) in grouped {
            let thread_id = ThreadId::from_string(owner).map_err(change_error)?;
            let ids: Vec<_> = ids.into_iter().map(str::to_owned).collect();
            let mut items = HashMap::with_capacity(ids.len());
            for batch in ids.chunks(THREAD_ITEMS_MAX_LIMIT) {
                let page = self
                    .thread_store
                    .list_items(ListItemsParams {
                        item_ids: Some(batch.to_vec()),
                        thread_id,
                        turn_id: Some(turn_id.to_string()),
                        include_archived: true,
                        position: None,
                        page_size: batch.len(),
                        sort_direction: SortDirection::Asc,
                        sort_key: ItemSortKey::CreatedAtOrdinal,
                        after_updated_at_ordinal: None,
                        before_updated_at_ordinal: None,
                    })
                    .await
                    .map_err(change_error)?;
                items.extend(
                    page.items
                        .into_iter()
                        .map(|item| (item.item_id.clone(), item)),
                );
            }
            evidence.insert((owner, turn_id), items);
        }
        Ok(evidence)
    }
}

fn change_error(error: impl std::fmt::Display) -> JSONRPCErrorError {
    invalid_request(format!("Captured edits cannot be reverted: {error}"))
}
