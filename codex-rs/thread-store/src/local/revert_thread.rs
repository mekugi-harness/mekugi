use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;

use codex_protocol::ThreadId;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HistoryPosition;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_rollout::RolloutConfig;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutRecorder;
use codex_rollout::RolloutRecorderParams;

use super::LocalThreadStore;
use super::paginated_fork;
use super::rollout_lineage::RolloutLineage;
use super::thread_history::find_source_turn;
use super::thread_history::find_visible_item;
use super::thread_history::has_earlier_user_message;
use super::thread_rollout_resolver;
use crate::ForkBoundary;
use crate::RevertThreadParams;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;

struct ResolvedRevertTarget {
    history_base: Option<HistoryPosition>,
    aborted_turn: Option<TurnAbortedEvent>,
}

/// Revert an unloaded paginated thread by creating a new immutable rollout file.
///
/// Old rollouts stay intact. The new file references the retained prefix, and the only mutable
/// cutover is the existing SQLite rollout-path pointer for the thread.
pub(super) async fn revert(
    store: &LocalThreadStore,
    params: RevertThreadParams,
) -> ThreadStoreResult<()> {
    let RevertThreadParams {
        thread_id,
        before_turn_id,
        before_item_id,
        multi_agent_version,
    } = params;
    if before_item_id.as_deref() == Some("") {
        return Err(ThreadStoreError::InvalidRequest {
            message: "beforeItemId must not be empty".to_string(),
        });
    }
    let state_db = store
        .state_db()
        .await
        .ok_or(ThreadStoreError::Unsupported {
            operation: "revert_thread",
        })?;
    let _lifecycle_guard = store.live_writer_locks.lock_lifecycle(thread_id).await;
    let _live_writer_guard = store.live_writer_locks.lock(thread_id).await;
    store.ensure_live_recorder_absent(thread_id).await?;
    let writer_lock = store.acquire_writer_lock(thread_id)?;

    // Resolution may return a compressed sibling. Keep SQLite's exact stored path for the CAS.
    let stored_metadata = state_db
        .get_thread(thread_id)
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to read thread metadata for {thread_id}: {err}"),
        })?
        .ok_or(ThreadStoreError::ThreadNotFound { thread_id })?;
    let expected_sqlite_path = stored_metadata.rollout_path;
    let current_rollout = thread_rollout_resolver::resolve_current(store, thread_id)
        .await?
        .ok_or(ThreadStoreError::ThreadNotFound { thread_id })?;
    let source_path = current_rollout.path;
    let mut source_meta = codex_rollout::read_session_meta_line(source_path.as_path())
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to read current paginated rollout {}: {err}",
                source_path.display()
            ),
        })?
        .meta;
    if source_meta.id != current_rollout.thread_id {
        return Err(ThreadStoreError::InvalidRequest {
            message: format!("current rollout for {thread_id} belongs to another thread"),
        });
    }
    if source_meta.history_mode != ThreadHistoryMode::Paginated {
        return Err(ThreadStoreError::InvalidRequest {
            message: format!("thread {thread_id} does not use paginated history"),
        });
    }

    // Older binaries can omit creator fields when replacing a rollout during revert.
    source_meta.creator_user_id = source_meta
        .creator_user_id
        .or(stored_metadata.creator_user_id);
    source_meta.creator_account_id = source_meta
        .creator_account_id
        .or(stored_metadata.creator_account_id);

    // Preserve old-reader compatibility when introducing the first reference to a standalone
    // source. Already-shared ancestors stay read-only; their offsets address decoded JSONL bytes.
    let lineage = store
        .resolve_rollout_lineage(thread_id, /*initial_path*/ None)
        .await?;
    let standalone_rollout_id = source_meta
        .history_base
        .is_none()
        .then_some(current_rollout.rollout_id);
    let ResolvedRevertTarget {
        history_base,
        aborted_turn,
    } = lineage
        .read_with_fallback(store, |mut lineage| {
            let before_turn_id = before_turn_id.clone();
            let before_item_id = before_item_id.clone();
            async move {
                for segment in &mut lineage.segments {
                    if Some(segment.rollout_id()) == standalone_rollout_id {
                        segment.rollout_path = codex_rollout::materialize_rollout_for_reference(
                            segment.rollout_path.as_path(),
                        )
                        .await
                        .map_err(|err| ThreadStoreError::Internal {
                            message: format!(
                                "failed to materialize rollout {} for revert: {err}",
                                segment.rollout_path.display()
                            ),
                        })?;
                    }
                    super::thread_history_materialization::materialize_to_sqlite_with_reads(
                        store,
                        segment.rollout_id(),
                        segment.rollout_path.as_path(),
                        &lineage.reads,
                    )
                    .await?;
                }
                resolve_revert_target(
                    store,
                    thread_id,
                    &before_turn_id,
                    before_item_id.as_deref(),
                    &lineage,
                )
                .await
            }
        })
        .await?;

    let forked_from_ordinal_exclusive =
        codex_rollout::forked_from_ordinal_exclusive(&source_meta, Some(source_path.as_path()))
            .map(|cutoff| {
                // Reverting into inherited history can shrink, but never grow, the parent prefix.
                cutoff.min(history_base.map_or(0, |base| base.end_ordinal_exclusive))
            });

    source_meta.multi_agent_version = multi_agent_version.or(source_meta.multi_agent_version);
    let rollout_id = ThreadId::new();
    let recorder = create_replacement_recorder(
        store,
        source_meta,
        rollout_id,
        history_base,
        forked_from_ordinal_exclusive,
        writer_lock,
    )
    .await?;
    let replacement_path = recorder.rollout_path().to_path_buf();
    let has_aborted_turn = aborted_turn.is_some();
    if let Some(aborted_turn) = aborted_turn {
        recorder
            .record_canonical_items(&[RolloutItem::EventMsg(EventMsg::TurnAborted(aborted_turn))])
            .await
            .map_err(thread_store_io_error)?;
    }
    recorder.persist().await.map_err(thread_store_io_error)?;
    recorder.shutdown().await.map_err(thread_store_io_error)?;

    let replaced = state_db
        .replace_rollout_path_if_current(
            thread_id,
            expected_sqlite_path.as_path(),
            replacement_path.as_path(),
        )
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to switch thread {thread_id} to reverted rollout: {err}"),
        })?;
    if !replaced {
        let _ = tokio::fs::remove_file(replacement_path.as_path()).await;
        return Err(ThreadStoreError::Conflict {
            message: format!("thread {thread_id} changed while it was being reverted"),
        });
    }
    if has_aborted_turn {
        super::thread_history_materialization::materialize_to_sqlite(
            store,
            rollout_id,
            replacement_path.as_path(),
        )
        .await?;
    }
    Ok(())
}

async fn resolve_revert_target(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    before_turn_id: &str,
    before_item_id: Option<&str>,
    lineage: &RolloutLineage,
) -> ThreadStoreResult<ResolvedRevertTarget> {
    let Some(before_item_id) = before_item_id else {
        let history_base = paginated_fork::history_base_at_boundary(
            store,
            thread_id,
            &ForkBoundary::BeforeTurn(before_turn_id.to_string()),
            lineage,
        )
        .await?;
        return Ok(ResolvedRevertTarget {
            history_base,
            aborted_turn: None,
        });
    };

    let pool = store.thread_history_db().await?;
    let visible = lineage.for_history_page().await?;
    let source_turn = find_source_turn(pool, &visible, before_turn_id).await?;
    let item = find_visible_item(pool, &visible, before_turn_id, before_item_id).await?;
    if !item.is_user_message {
        return Err(ThreadStoreError::InvalidRequest {
            message: format!(
                "item {before_item_id} in turn {before_turn_id} is not a user message"
            ),
        });
    }
    if !has_earlier_user_message(pool, &visible, before_turn_id, item.rollout_ordinal).await? {
        let history_base = paginated_fork::history_base_at_boundary(
            store,
            thread_id,
            &ForkBoundary::BeforeTurn(before_turn_id.to_string()),
            lineage,
        )
        .await?;
        return Ok(ResolvedRevertTarget {
            history_base,
            aborted_turn: None,
        });
    }

    let segment = visible
        .segments()
        .iter()
        .find(|segment| segment.rollout_id() == item.rollout_id)
        .ok_or_else(|| ThreadStoreError::Internal {
            message: "item segment is outside the source lineage".to_string(),
        })?;
    let scan_start_offset = if source_turn.rollout_id == item.rollout_id {
        source_turn
            .rollout_byte_offset
            .map(|offset| {
                u64::try_from(offset).map_err(|_| ThreadStoreError::Internal {
                    message: format!("invalid rollout byte offset for turn {before_turn_id}"),
                })
            })
            .transpose()?
            .unwrap_or(0)
    } else {
        0
    };
    let end_byte_offset = segment.end.map(|end| end.end_byte_offset);
    let rollout_path = segment.rollout_path.clone();
    let reads = visible.reads.clone();
    let turn_id_owned = before_turn_id.to_string();
    let item_id_owned = before_item_id.to_string();
    let target_ordinal = item.rollout_ordinal;
    let (stop, keep_running) = tokio::sync::oneshot::channel::<()>();
    let (cut_ordinal, cut_byte_offset) = tokio::task::spawn_blocking(move || {
        locate_user_message_cut_position(
            &reads,
            rollout_path.as_path(),
            scan_start_offset,
            end_byte_offset,
            &turn_id_owned,
            &item_id_owned,
            target_ordinal,
            || stop.is_closed(),
        )
    })
    .await
    .map_err(|err| ThreadStoreError::Internal {
        message: format!("failed to join mid-turn revert scan: {err}"),
    })??;
    drop(keep_running);

    let history_base = paginated_fork::normalize_history_base(
        lineage,
        HistoryPosition {
            thread_id: item.rollout_id,
            end_ordinal_exclusive: cut_ordinal,
            end_byte_offset: cut_byte_offset,
        },
    )?;
    Ok(ResolvedRevertTarget {
        history_base,
        aborted_turn: Some(TurnAbortedEvent {
            turn_id: Some(before_turn_id.to_string()),
            root_turn_id: source_turn.root_turn_id,
            reason: TurnAbortReason::Interrupted,
            error: None,
            started_at: source_turn.started_at,
            completed_at: None,
            duration_ms: None,
        }),
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "blocking rollout scan keeps segment bounds and target identifiers explicit"
)]
fn locate_user_message_cut_position(
    reads: &super::rollout_lineage::AncestorReads,
    rollout_path: &std::path::Path,
    start_offset: u64,
    end_byte_offset: Option<u64>,
    turn_id: &str,
    item_id: &str,
    target_ordinal: u64,
    is_cancelled: impl Fn() -> bool,
) -> ThreadStoreResult<(u64, u64)> {
    let mut file = reads
        .open(rollout_path, &is_cancelled)
        .map_err(thread_store_io_error)?;
    let file_end_offset = match end_byte_offset {
        Some(end) => end,
        None => file.seek(SeekFrom::End(0)).map_err(thread_store_io_error)?,
    };
    let byte_count = file_end_offset.checked_sub(start_offset).ok_or_else(|| {
        reads.record_failure(rollout_path);
        ThreadStoreError::Internal {
            message: format!(
                "durable rollout shrank before revert scan: {}",
                rollout_path.display()
            ),
        }
    })?;
    file.seek(SeekFrom::Start(start_offset))
        .map_err(thread_store_io_error)?;
    let mut reader = BufReader::new(file.take(byte_count));
    let mut line_bytes = Vec::new();
    let mut line_start_offset = start_offset;
    let mut pending_user_boundary: Option<(u64, u64)> = None;

    loop {
        if is_cancelled() {
            return Err(thread_store_io_error(std::io::Error::other(
                "rollout revert scan cancelled",
            )));
        }
        line_bytes.clear();
        if reader
            .read_until(b'\n', &mut line_bytes)
            .map_err(thread_store_io_error)?
            == 0
        {
            break;
        }
        if !line_bytes.ends_with(b"\n") {
            break;
        }
        let current_start = line_start_offset;
        line_start_offset = current_start
            .checked_add(u64::try_from(line_bytes.len()).map_err(|_| {
                ThreadStoreError::Internal {
                    message: "durable rollout byte offset overflow".to_string(),
                }
            })?)
            .ok_or_else(|| ThreadStoreError::Internal {
                message: "durable rollout byte offset overflow".to_string(),
            })?;
        if line_bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line_bytes) else {
            continue;
        };
        let Ok(line) = codex_rollout::decode_rollout_line(value) else {
            continue;
        };
        let Some(ordinal) = line.ordinal else {
            continue;
        };
        if ordinal > target_ordinal {
            break;
        }
        match &line.item {
            RolloutItem::ResponseItem(envelope) if matches!(&envelope.item, ResponseItem::Message { role, .. } if role == "user") =>
            {
                pending_user_boundary = Some((ordinal, current_start));
            }
            RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                pending_user_boundary.get_or_insert((ordinal, current_start));
            }
            RolloutItem::EventMsg(EventMsg::ItemStarted(event))
                if event.turn_id == turn_id
                    && matches!(&event.item, TurnItem::UserMessage(user) if user.id == item_id) =>
            {
                pending_user_boundary.get_or_insert((ordinal, current_start));
            }
            RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) => {
                if ordinal == target_ordinal {
                    if event.turn_id == turn_id
                        && matches!(&event.item, TurnItem::UserMessage(user) if user.id == item_id)
                    {
                        return Ok(pending_user_boundary.unwrap_or((ordinal, current_start)));
                    }
                    return Err(ThreadStoreError::InvalidRequest {
                        message: format!(
                            "item {item_id} in turn {turn_id} is not a persisted user message"
                        ),
                    });
                }
                if matches!(&event.item, TurnItem::UserMessage(_)) {
                    pending_user_boundary = None;
                }
            }
            RolloutItem::EventMsg(
                EventMsg::TurnStarted(_) | EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_),
            ) => {
                pending_user_boundary = None;
            }
            _ => {}
        }
    }

    Err(ThreadStoreError::InvalidRequest {
        message: format!(
            "item {item_id} in turn {turn_id} does not have a persisted rollout boundary"
        ),
    })
}

async fn create_replacement_recorder(
    store: &LocalThreadStore,
    source_meta: codex_rollout::SessionMeta,
    rollout_id: ThreadId,
    history_base: Option<codex_protocol::protocol::HistoryPosition>,
    forked_from_ordinal_exclusive: Option<u64>,
    writer_lock: super::WriterLockGuard,
) -> ThreadStoreResult<RolloutRecorder> {
    let config = RolloutConfig {
        codex_home: store.config.codex_home.clone(),
        sqlite: store.config.sqlite.clone(),
        cwd: source_meta.cwd.clone(),
        model_provider_id: source_meta
            .model_provider
            .clone()
            .unwrap_or_else(|| store.config.default_model_provider_id.clone()),
        generate_memories: source_meta.memory_mode.as_deref() != Some("disabled"),
    };
    let mut params = RolloutRecorderParams::new(
        source_meta.id,
        source_meta.forked_from_id,
        source_meta.parent_thread_id,
        source_meta.source,
        source_meta.thread_source,
        source_meta.originator,
        source_meta.base_instructions.unwrap_or_default(),
        source_meta.dynamic_tools.unwrap_or_default(),
    )
    .with_creator(
        source_meta.creator_user_id,
        source_meta.creator_account_id,
        source_meta.account_scoped,
    )
    .with_session_id(source_meta.session_id)
    .with_attached_thread(source_meta.attached_thread)
    .with_rollout_id(rollout_id)
    .with_selected_capability_roots(source_meta.selected_capability_roots)
    .with_runtime_workspace_roots(source_meta.runtime_workspace_roots)
    .with_multi_agent_version(source_meta.multi_agent_version)
    .with_history_mode(ThreadHistoryMode::Paginated)
    .with_history_base(history_base)
    .with_forked_from_ordinal_exclusive(forked_from_ordinal_exclusive)
    .with_subagent_history_start_ordinal(source_meta.subagent_history_start_ordinal);
    if let Some(context_window) = source_meta.context_window {
        params = params.with_initial_window_id(context_window.window_id);
    }
    RolloutRecorder::new_with_writer_lock(&config, params, writer_lock)
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to create reverted rollout: {err}"),
        })
}

fn thread_store_io_error(err: std::io::Error) -> ThreadStoreError {
    ThreadStoreError::Internal {
        message: err.to_string(),
    }
}

#[cfg(test)]
#[path = "revert_thread_tests.rs"]
mod tests;
