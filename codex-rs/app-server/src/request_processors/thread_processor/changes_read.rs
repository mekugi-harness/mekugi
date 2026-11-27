use std::collections::BTreeMap;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ThreadChangeCallerCounts;
use codex_app_server_protocol::ThreadChangeFileDiff;
use codex_app_server_protocol::ThreadChangeOrigin;
use codex_app_server_protocol::ThreadChangesLiveParams;
use codex_app_server_protocol::ThreadChangesLiveResponse;
use codex_app_server_protocol::ThreadChangesReadParams;
use codex_app_server_protocol::ThreadChangesReadResponse;
use codex_change_history::ChangeAuthor;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeRecord;
use codex_change_history::compose_diff;
use codex_change_history::compose_file_stats;
use codex_protocol::ThreadId;

use super::ThreadRequestProcessor;
use super::thread_read_view_error;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;

impl ThreadRequestProcessor {
    #[tracing::instrument(level = "debug", skip(self))]
    pub(crate) async fn thread_changes_live(
        &self,
        params: ThreadChangesLiveParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id = ThreadId::from_string(&params.thread_id)
            .map_err(|error| invalid_request(format!("invalid thread ID: {error}")))?;
        let data = if let Ok(thread) = self.thread_manager.get_thread(id).await {
            self.thread_state_manager
                .live_command_changes(id, &thread)
                .await
        } else {
            self.read_stored_thread_for_read(id, /*include_history*/ false)
                .await
                .map_err(thread_read_view_error)?
                .ok_or_else(|| invalid_request(format!("unknown thread: {id}")))?;
            Vec::new()
        };
        Ok(Some(ThreadChangesLiveResponse { data }.into()))
    }

    pub(crate) async fn thread_changes_read(
        &self,
        params: ThreadChangesReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id = ThreadId::from_string(&params.thread_id)
            .map_err(|error| invalid_request(format!("invalid thread ID: {error}")))?;
        let (root, workspace) = if let Ok(thread) = self.thread_manager.get_thread(id).await {
            (
                thread.agent_root_id(),
                Some(thread.config().await.cwd.display().to_string()),
            )
        } else {
            let mut root = id;
            let mut workspace = None;
            loop {
                let stored = self
                    .read_stored_thread_for_read(root, /*include_history*/ false)
                    .await
                    .map_err(thread_read_view_error)?
                    .ok_or_else(|| invalid_request(format!("unknown thread: {root}")))?;
                if workspace.is_none() {
                    workspace = Some(stored.cwd.display().to_string());
                }
                match stored.parent_thread_id {
                    Some(parent) => root = parent,
                    None => break,
                }
            }
            (root, workspace)
        };
        let member_scope = if root == id && params.item_id.is_none() {
            None
        } else {
            Some(id.to_string())
        };
        let dir =
            codex_protocol::shell_environment::session_tmp_path(root.into()).join("change-history");
        let cursor = params.cursor.as_deref().map(parse_cursor).transpose()?;
        let response = tokio::task::spawn_blocking(move || {
            read_changes(dir, workspace, member_scope, params, cursor)
        })
        .await
        .map_err(|error| internal_error(format!("could not read captured edits: {error}")))??;
        Ok(Some(ClientResponsePayload::ThreadChangesRead(response)))
    }
}

fn parse_cursor(value: &str) -> Result<(u64, usize), JSONRPCErrorError> {
    let Some((sequence, offset)) = value.split_once(':') else {
        return Err(invalid_request("invalid captured edit cursor"));
    };
    sequence
        .parse()
        .ok()
        .zip(offset.parse().ok())
        .ok_or_else(|| invalid_request("invalid captured edit cursor"))
}

fn read_changes(
    dir: PathBuf,
    workspace: Option<String>,
    member_scope: Option<String>,
    params: ThreadChangesReadParams,
    cursor: Option<(u64, usize)>,
) -> Result<ThreadChangesReadResponse, JSONRPCErrorError> {
    let read_error = |error| internal_error(format!("could not read captured edits: {error}"));
    let mut response = ThreadChangesReadResponse {
        thread_id: params.thread_id,
        workspace,
        changes: Vec::new(),
        files: Vec::new(),
        file_count: 0,
        next_cursor: None,
        incomplete: false,
        callers: Vec::new(),
        total_additions: 0,
        total_deletions: 0,
    };
    match std::fs::metadata(&dir) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(response),
        Err(error) => {
            return Err(internal_error(format!(
                "could not inspect captured edits: {error}"
            )));
        }
    }
    let store = ChangeHistoryStore::open_existing(&dir).map_err(read_error)?;
    let entries = store.list_entries().map_err(read_error)?;
    let (sequence, offset) =
        cursor.unwrap_or_else(|| (entries.last().map_or(0, |entry| entry.sequence), 0));
    let include_external = params.include_external.unwrap_or(true);
    let include_ephemeral = params.include_ephemeral.unwrap_or(false);
    let ids: Vec<_> = entries
        .into_iter()
        .filter(|entry| {
            entry.sequence <= sequence
                && entry.source_kind.is_authored()
                && (include_ephemeral || !entry.ephemeral_only)
                && member_scope
                    .as_ref()
                    .is_none_or(|id| *id == entry.thread_id)
                && params
                    .item_id
                    .as_ref()
                    .is_none_or(|id| *id == entry.call_id)
        })
        .map(|entry| entry.id)
        .collect();
    if ids.is_empty() {
        return Ok(response);
    }
    let mut records = store.load_records(&ids).map_err(read_error)?;
    records.retain_mut(|record| {
        record.files.retain(|file| match file.scope {
            codex_change_history::ChangeScope::Workspace => true,
            codex_change_history::ChangeScope::External => include_external,
            codex_change_history::ChangeScope::Ephemeral => include_ephemeral,
        });
        !record.files.is_empty()
    });
    if let Some(caller) = params.caller_thread_id.as_ref() {
        records.retain(|record| author_thread(record).as_ref() == Some(caller));
    }
    let separate = params.per_capture || params.caller_thread_id.is_some();
    let mut composed = Vec::new();
    if separate {
        for record in records.iter().rev() {
            let effects =
                compose_diff(&store, std::slice::from_ref(record), &[]).map_err(read_error)?;
            composed.extend(effects);
        }
    } else {
        composed = compose_diff(&store, &records, &[]).map_err(read_error)?;
        let sequence_by_id: BTreeMap<_, _> = records
            .iter()
            .map(|record| (&record.id, record.sequence))
            .collect();
        composed.sort_by_cached_key(|file| {
            std::cmp::Reverse(
                file.stats
                    .change_ids
                    .last()
                    .and_then(|id| sequence_by_id.get(id)),
            )
        });
    }
    response.file_count = composed.len() as u32;
    response.total_additions = composed
        .iter()
        .map(|diff| diff.stats.additions as i64)
        .sum();
    response.total_deletions = composed
        .iter()
        .map(|diff| diff.stats.deletions as i64)
        .sum();
    response.incomplete = composed
        .iter()
        .any(|diff| !diff.stats.coverage_notes.is_empty());
    // The upper sequence keeps later pages on the same append-only snapshot.
    let limit = params
        .limit
        .map_or(usize::MAX, |limit| limit.clamp(1, 100) as usize);
    let end = offset.saturating_add(limit);
    if end < composed.len() {
        response.next_cursor = Some(format!("{sequence}:{end}"));
    }
    let mut page_ids = HashSet::new();
    for diff in composed.into_iter().skip(offset).take(limit) {
        let stats = diff.stats;
        page_ids.extend(stats.change_ids.iter().cloned());
        response.files.push(ThreadChangeFileDiff {
            path: stats.rel_path,
            absolute_path: stats.abs_path.display().to_string(),
            change_ids: stats
                .change_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            moved_from: stats.moved_from,
            status: stats.status.to_string(),
            unified_diff: diff.unified_diff,
            additions: stats.additions as i64,
            deletions: stats.deletions as i64,
            is_binary: stats.is_binary,
            coverage_notes: stats.coverage_notes,
            scope: stats.scope.as_str().to_string(),
        });
    }
    let all_origins = params.limit.is_none() && cursor.is_none();
    let caller_index = cursor.is_none() && !separate && params.item_id.is_none();
    let mut callers = BTreeMap::<String, Vec<ChangeRecord>>::new();
    for record in records {
        let author = author_thread(&record);
        let show = all_origins || page_ids.contains(&record.id);
        if show {
            let (source_kind, source_label) = record.metadata.source_kind.review_source();
            response.changes.push(ThreadChangeOrigin {
                change_id: record.id.to_string(),
                sequence: record.sequence,
                thread_id: record.metadata.thread_id.clone(),
                author_thread_id: author.clone(),
                turn_id: record.metadata.turn_id.clone(),
                item_id: record.metadata.call_id.clone(),
                workspace: record.metadata.workspace_root.display().to_string(),
                source_kind: source_kind.to_string(),
                source_label: source_label.to_string(),
                created_at: record.metadata.created_at,
                paths: record
                    .files
                    .iter()
                    .map(|file| file.rel_path.clone())
                    .collect(),
                incomplete: record.files.iter().any(|file| !file.coverage.is_complete()),
            });
        }
        if caller_index && let Some(author) = author {
            callers.entry(author).or_default().push(record);
        }
    }
    for (thread_id, records) in callers {
        let workspaces: BTreeMap<_, _> = records
            .iter()
            .map(|record| (&record.id, &record.metadata.workspace_root))
            .collect();
        let files = compose_file_stats(&store, &records, &[]).map_err(read_error)?;
        let (mut additions, mut deletions) = (0, 0);
        for file in &files {
            if file.coverage_notes.is_empty()
                && file
                    .change_ids
                    .last()
                    .and_then(|id| workspaces.get(id))
                    .is_some_and(|workspace| file.abs_path.starts_with(workspace))
            {
                additions += file.additions as i64;
                deletions += file.deletions as i64;
            }
        }
        response.callers.push(ThreadChangeCallerCounts {
            thread_id,
            file_count: files.len() as u32,
            additions,
            deletions,
        });
    }
    Ok(response)
}

fn author_thread(record: &ChangeRecord) -> Option<String> {
    match record.metadata.effective_author() {
        ChangeAuthor::Agent { thread_id } => Some(thread_id),
        ChangeAuthor::Unattributed => None,
    }
}
