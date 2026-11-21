use codex_protocol::ThreadId;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::AgentMessageItem;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::user_input::UserInput;
use codex_rollout::RolloutItem;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::super::LocalThreadStore;
use super::super::test_support::test_config;
use crate::AppendThreadItemsParams;
use crate::ArchiveThreadParams;
use crate::CreateThreadParams;
use crate::DeleteThreadParams;
use crate::ForkBoundary;
use crate::ItemSortKey;
use crate::ListItemsParams;
use crate::ListTurnsParams;
use crate::LoadThreadHistoryParams;
use crate::LocalHistoryAccount;
use crate::PrepareForkParams;
use crate::RevertThreadParams;
use crate::SortDirection;
use crate::StoredTurnItemsView;
use crate::StoredTurnStatus;
use crate::ThreadPersistenceMetadata;
use crate::ThreadStore;
use crate::ThreadStoreError;

#[tokio::test]
async fn revert_keeps_thread_id_and_hides_suffix_across_repeated_reverts() {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let state_db = codex_state::StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("initialize state database");
    let store = LocalThreadStore::new(config, Some(state_db.clone()));
    let thread_id = ThreadId::new();
    create_paginated_thread(&store, thread_id).await;
    store
        .append_items(AppendThreadItemsParams {
            thread_id,
            items: vec![
                turn_started("turn-1"),
                turn_completed("turn-1"),
                turn_started("turn-2"),
                turn_completed("turn-2"),
            ],
        })
        .await
        .expect("append turns");
    let original_path = store
        .live_rollout_path(thread_id)
        .await
        .expect("source rollout path");
    store
        .shutdown_thread(thread_id)
        .await
        .expect("close source writer");
    codex_rollout::state_db::reconcile_rollout(
        Some(state_db.as_ref()),
        original_path.as_path(),
        "test-provider",
        /*builder*/ None,
        &[],
        /*archived_only*/ Some(false),
        /*new_thread_memory_mode*/ None,
    )
    .await;
    compress_rollout(original_path.as_path());

    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-2".to_string(),
            before_item_id: None,
            multi_agent_version: None,
        })
        .await
        .expect("revert before second turn");
    let first_replacement_path = state_db
        .get_thread(thread_id)
        .await
        .expect("read metadata")
        .expect("thread metadata")
        .rollout_path;
    assert_ne!(first_replacement_path, original_path);
    assert_ne!(
        codex_rollout::rollout_id_from_path(first_replacement_path.as_path()),
        Some(thread_id)
    );
    let replacement_meta = codex_rollout::read_session_meta_line(first_replacement_path.as_path())
        .await
        .expect("read replacement metadata")
        .meta;
    assert_eq!(replacement_meta.id, thread_id);
    assert_eq!(
        (
            replacement_meta.account_scoped,
            replacement_meta.creator_user_id.as_deref(),
            replacement_meta.creator_account_id.as_deref()
        ),
        (true, Some("creator-user"), Some("creator-account")),
    );
    assert_eq!(replacement_meta.memory_mode, None);
    assert_eq!(
        replacement_meta.runtime_workspace_roots,
        Some(vec![home.path().join("workspace")])
    );
    assert_eq!(turn_ids(&store, thread_id).await, vec!["turn-1"]);

    // Simulate an older binary replacing the rollout without the creator fields it does not know.
    let rollout = tokio::fs::read_to_string(&first_replacement_path)
        .await
        .expect("read replacement rollout");
    let (meta_line, remaining_lines) = rollout.split_once('\n').expect("session metadata line");
    let mut meta_line: serde_json::Value = serde_json::from_str(meta_line).expect("parse metadata");
    let payload = meta_line["payload"]
        .as_object_mut()
        .expect("metadata payload");
    payload.remove("account_scoped");
    payload.remove("creator_user_id");
    payload.remove("creator_account_id");
    tokio::fs::write(
        &first_replacement_path,
        format!("{meta_line}\n{remaining_lines}"),
    )
    .await
    .expect("write legacy replacement rollout");

    for (user, expected) in [("creator-user", true), ("other-user", false)] {
        let account = LocalHistoryAccount {
            user_id: Some(user.to_string()),
            account_id: Some("creator-account".to_string()),
        };
        assert_eq!(
            account
                .includes(
                    home.path(),
                    thread_id,
                    Some(&first_replacement_path),
                    &[thread_id],
                    /*original_paths*/ &mut None,
                )
                .await,
            expected
        );
    }

    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-1".to_string(),
            before_item_id: None,
            multi_agent_version: None,
        })
        .await
        .expect("revert before first turn");
    assert_eq!(turn_ids(&store, thread_id).await, Vec::<String>::new());
    let stored = state_db
        .get_thread(thread_id)
        .await
        .expect("read metadata")
        .expect("thread metadata");
    let recovered = codex_rollout::read_session_meta_line(&stored.rollout_path)
        .await
        .expect("read recovered creator metadata")
        .meta;
    assert_eq!(
        (recovered.creator_user_id, recovered.creator_account_id),
        (
            Some("creator-user".to_string()),
            Some("creator-account".to_string())
        ),
    );

    store
        .archive_thread(ArchiveThreadParams { thread_id })
        .await
        .expect("archive reverted thread");
    assert!(
        rollout_paths_for_thread(home.path(), thread_id)
            .await
            .iter()
            .all(|path| path.starts_with(home.path().join("archived_sessions")))
    );
    store
        .unarchive_thread(ArchiveThreadParams { thread_id })
        .await
        .expect("unarchive reverted thread");
    let owned_rollout_paths = rollout_paths_for_thread(home.path(), thread_id).await;
    assert_eq!(owned_rollout_paths.len(), 3);
    assert!(
        owned_rollout_paths
            .iter()
            .all(|path| path.starts_with(home.path().join("sessions")))
    );

    store
        .delete_thread(DeleteThreadParams { thread_id })
        .await
        .expect("delete reverted thread");
    for rollout_path in owned_rollout_paths {
        assert!(!rollout_path.exists());
    }
}

#[tokio::test]
async fn revert_preserves_legacy_scope_and_resolved_multi_agent_version() {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let state_db = codex_state::StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("initialize state database");
    let store = LocalThreadStore::new(config, Some(state_db.clone()));
    let other_account = LocalHistoryAccount {
        user_id: Some("other-user".to_string()),
        account_id: Some("other-account".to_string()),
    };
    let mut archived_threads = Vec::new();

    for version in [
        MultiAgentVersion::V1,
        MultiAgentVersion::V2,
        MultiAgentVersion::Disabled,
    ] {
        let thread_id = ThreadId::new();
        create_paginated_thread(&store, thread_id).await;
        store
            .append_items(AppendThreadItemsParams {
                thread_id,
                items: vec![
                    turn_started("turn-1"),
                    turn_completed("turn-1"),
                    turn_started("turn-2"),
                    turn_completed("turn-2"),
                ],
            })
            .await
            .expect("append turns");
        let path = store
            .live_rollout_path(thread_id)
            .await
            .expect("source rollout path");
        store
            .shutdown_thread(thread_id)
            .await
            .expect("close source writer");
        let contents = tokio::fs::read_to_string(&path)
            .await
            .expect("read rollout");
        // Preserve indexed offsets while turning the fixture into pre-enforcement history.
        let marker = "\"account_scoped\":true,";
        tokio::fs::write(
            &path,
            contents.replacen(marker, &" ".repeat(marker.len()), 1),
        )
        .await
        .expect("write legacy rollout");
        codex_rollout::state_db::reconcile_rollout(
            Some(state_db.as_ref()),
            path.as_path(),
            "test-provider",
            /*builder*/ None,
            &[],
            /*archived_only*/ Some(false),
            /*new_thread_memory_mode*/ None,
        )
        .await;

        // The second revert has no live version to supply and must keep the persisted one.
        for (before_turn_id, multi_agent_version) in [("turn-2", Some(version)), ("turn-1", None)] {
            store
                .revert_thread(RevertThreadParams {
                    thread_id,
                    before_turn_id: before_turn_id.to_string(),
                    before_item_id: None,
                    multi_agent_version,
                })
                .await
                .expect("revert thread");
            let path = state_db
                .get_thread(thread_id)
                .await
                .expect("read metadata")
                .expect("thread metadata")
                .rollout_path;
            let meta = codex_rollout::read_session_meta_line(&path)
                .await
                .expect("read replacement metadata");
            assert_eq!(
                (meta.meta.multi_agent_version, meta.meta.account_scoped),
                (Some(version), false)
            );
            assert!(
                other_account
                    .includes(
                        home.path(),
                        thread_id,
                        Some(&path),
                        &[thread_id],
                        /*original_paths*/ &mut None,
                    )
                    .await
            );
        }
        store
            .archive_thread(ArchiveThreadParams { thread_id })
            .await
            .expect("archive reverted legacy thread");
        let path = state_db
            .get_thread(thread_id)
            .await
            .expect("read archived metadata")
            .expect("archived thread")
            .rollout_path;
        archived_threads.push((thread_id, path));
    }
    let page_thread_ids: Vec<_> = archived_threads.iter().map(|(id, _)| *id).collect();
    let mut original_paths = None;
    for (thread_id, path) in archived_threads {
        assert!(
            other_account
                .includes(
                    home.path(),
                    thread_id,
                    Some(&path),
                    &page_thread_ids,
                    &mut original_paths,
                )
                .await
        );
    }
}

async fn rollout_paths_for_thread(
    home: &std::path::Path,
    thread_id: ThreadId,
) -> Vec<std::path::PathBuf> {
    codex_rollout::RolloutReferenceIndex::scan(home)
        .await
        .expect("scan rollout references")
        .rollouts_for_thread(thread_id)
        .map(|(_, path)| path.to_path_buf())
        .collect()
}

async fn create_paginated_thread(store: &LocalThreadStore, thread_id: ThreadId) {
    store
        .create_thread(CreateThreadParams {
            attached_thread: false,
            creator_user_id: Some("creator-user".to_string()),
            creator_account_id: Some("creator-account".to_string()),
            session_id: thread_id.into(),
            thread_id,
            extra_config: None,
            forked_from_id: None,
            parent_thread_id: None,
            source: SessionSource::Exec,
            thread_source: None,
            originator: "test_originator".to_string(),
            base_instructions: BaseInstructions::default(),
            dynamic_tools: Vec::new(),
            selected_capability_roots: Vec::new(),
            multi_agent_version: None,
            history_mode: ThreadHistoryMode::Paginated,
            history_base: None,
            subagent_history_start_ordinal: None,
            initial_window_id: codex_protocol::ContextWindowId::new_uuid_v7(),
            runtime_workspace_roots: Some(vec![store.config.codex_home.join("workspace").abs()]),
            metadata: ThreadPersistenceMetadata {
                cwd: Some(std::env::current_dir().expect("cwd")),
                model_provider: "test-provider".to_string(),
                memory_mode: ThreadMemoryMode::Enabled,
            },
        })
        .await
        .expect("create paginated thread");
}

async fn turn_ids(store: &LocalThreadStore, thread_id: ThreadId) -> Vec<String> {
    store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::NotLoaded,
        })
        .await
        .expect("list turns")
        .turns
        .into_iter()
        .map(|turn| turn.turn_id)
        .collect()
}

fn turn_started(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_attribution: None,
        turn_id: turn_id.to_string(),
        root_turn_id: None,
        trace_id: None,
        started_at: Some(10),
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    }))
}

fn turn_completed(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
        root_turn_id: None,
        turn_id: turn_id.to_string(),
        last_agent_message: None,
        error: None,
        started_at: Some(10),
        completed_at: Some(20),
        duration_ms: Some(10_000),
        time_to_first_token_ms: None,
    }))
}

fn compress_rollout(path: &std::path::Path) {
    let contents = std::fs::read(path).expect("read rollout");
    let compressed = zstd::stream::encode_all(contents.as_slice(), 3).expect("compress rollout");
    std::fs::write(path.with_extension("jsonl.zst"), compressed).expect("write compressed rollout");
    std::fs::remove_file(path).expect("remove plain rollout");
}

#[tokio::test]
async fn revert_before_steered_user_message_preserves_prefix_and_closes_turn() {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let state_db = codex_state::StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("initialize state database");
    let store = LocalThreadStore::new(config, Some(state_db.clone()));
    let thread_id = ThreadId::new();
    create_paginated_thread(&store, thread_id).await;
    store
        .append_items(AppendThreadItemsParams {
            thread_id,
            items: vec![
                turn_started("turn-1"),
                user_response("initial prompt"),
                user_item_completed(thread_id, "turn-1", "u1", "initial prompt"),
                assistant_response("first reply"),
                agent_item_completed(thread_id, "turn-1", "a1", "first reply"),
                user_response("first steer"),
                user_item_completed(thread_id, "turn-1", "u2", "first steer"),
                assistant_response("second reply"),
                agent_item_completed(thread_id, "turn-1", "a2", "second reply"),
                user_response("second steer"),
                user_item_completed(thread_id, "turn-1", "u3", "second steer"),
                assistant_response("third reply"),
                agent_item_completed(thread_id, "turn-1", "a3", "third reply"),
                turn_completed("turn-1"),
                turn_started("turn-2"),
                user_response("later turn"),
                user_item_completed(thread_id, "turn-2", "u4", "later turn"),
                turn_completed("turn-2"),
            ],
        })
        .await
        .expect("append steered turns");
    let original_path = store
        .live_rollout_path(thread_id)
        .await
        .expect("source rollout path");
    store
        .shutdown_thread(thread_id)
        .await
        .expect("close source writer");
    codex_rollout::state_db::reconcile_rollout(
        Some(state_db.as_ref()),
        original_path.as_path(),
        "test-provider",
        /*builder*/ None,
        &[],
        /*archived_only*/ Some(false),
        /*new_thread_memory_mode*/ None,
    )
    .await;
    compress_rollout(original_path.as_path());

    // Invalid targets fail cleanly without changing retained history.
    for (bad_item, expected_fragment) in [
        ("", "beforeItemId must not be empty"),
        ("missing-item", "item not found: missing-item"),
        ("a1", "is not a user message"),
    ] {
        let err = store
            .revert_thread(RevertThreadParams {
                thread_id,
                before_turn_id: "turn-1".to_string(),
                before_item_id: Some(bad_item.to_string()),
                multi_agent_version: None,
            })
            .await
            .expect_err("invalid before_item_id should fail");
        assert!(
            matches!(&err, ThreadStoreError::InvalidRequest { message } if message.contains(expected_fragment)),
            "unexpected error for {bad_item:?}: {err:?}"
        );
    }
    assert_eq!(turn_ids(&store, thread_id).await, vec!["turn-1", "turn-2"]);

    // Revert mid-turn before the second steer (`u3`).
    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-1".to_string(),
            before_item_id: Some("u3".to_string()),
            multi_agent_version: None,
        })
        .await
        .expect("revert before second steer");
    let turns = store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::Summary,
        })
        .await
        .expect("list summary turns");
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].turn_id, "turn-1");
    assert_eq!(turns.turns[0].status, StoredTurnStatus::Interrupted);
    assert_eq!(
        turns.turns[0]
            .items
            .iter()
            .map(|item| item.item_id.as_str())
            .collect::<Vec<_>>(),
        vec!["u1"]
    );
    assert_eq!(
        turn_item_ids(&store, thread_id, "turn-1").await,
        vec!["u1", "a1", "u2", "a2"]
    );
    assert_eq!(
        model_message_texts(&store, thread_id).await,
        vec![
            "initial prompt",
            "first reply",
            "first steer",
            "second reply"
        ]
    );

    // Forking through the interrupted turn after a mid-turn revert succeeds.
    store
        .prepare_fork(PrepareForkParams {
            thread_id,
            boundary: ForkBoundary::ThroughTurn("turn-1".to_string()),
        })
        .await
        .expect("fork through mid-turn reverted turn");

    // Repeated mid-turn revert to the earlier steer (`u2`).
    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-1".to_string(),
            before_item_id: Some("u2".to_string()),
            multi_agent_version: None,
        })
        .await
        .expect("revert before first steer");
    assert_eq!(
        turn_item_ids(&store, thread_id, "turn-1").await,
        vec!["u1", "a1"]
    );
    assert_eq!(
        model_message_texts(&store, thread_id).await,
        vec!["initial prompt", "first reply"]
    );

    // Reverting before the turn's initial user message (`u1`) reverts the entire turn.
    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-1".to_string(),
            before_item_id: Some("u1".to_string()),
            multi_agent_version: None,
        })
        .await
        .expect("revert before initial user message");
    assert_eq!(turn_ids(&store, thread_id).await, Vec::<String>::new());
    assert_eq!(
        model_message_texts(&store, thread_id).await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn revert_before_steered_user_message_survives_interleaved_items() {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let state_db = codex_state::StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("initialize state database");
    let store = LocalThreadStore::new(config, Some(state_db.clone()));
    let thread_id = ThreadId::new();
    create_paginated_thread(&store, thread_id).await;
    store
        .append_items(AppendThreadItemsParams {
            thread_id,
            items: vec![
                turn_started("turn-1"),
                user_response("initial prompt"),
                user_item_completed(thread_id, "turn-1", "u1", "initial prompt"),
                assistant_response("first reply"),
                agent_item_completed(thread_id, "turn-1", "a1", "first reply"),
                // Concurrent non-user records land between the steer's ResponseItem and ItemCompleted.
                user_response("interleaved steer"),
                assistant_response("concurrent progress"),
                agent_item_completed(thread_id, "turn-1", "a-concurrent", "concurrent progress"),
                user_item_completed(thread_id, "turn-1", "u2", "interleaved steer"),
                assistant_response("steered reply"),
                agent_item_completed(thread_id, "turn-1", "a2", "steered reply"),
                turn_completed("turn-1"),
            ],
        })
        .await
        .expect("append interleaved steered turn");
    let original_path = store
        .live_rollout_path(thread_id)
        .await
        .expect("source rollout path");
    store
        .shutdown_thread(thread_id)
        .await
        .expect("close source writer");
    codex_rollout::state_db::reconcile_rollout(
        Some(state_db.as_ref()),
        original_path.as_path(),
        "test-provider",
        /*builder*/ None,
        &[],
        /*archived_only*/ Some(false),
        /*new_thread_memory_mode*/ None,
    )
    .await;

    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-1".to_string(),
            before_item_id: Some("u2".to_string()),
            multi_agent_version: None,
        })
        .await
        .expect("revert before interleaved steer");
    assert_eq!(
        turn_item_ids(&store, thread_id, "turn-1").await,
        vec!["u1", "a1"]
    );
    assert_eq!(
        model_message_texts(&store, thread_id).await,
        vec!["initial prompt", "first reply"]
    );
}

async fn turn_item_ids(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    turn_id: &str,
) -> Vec<String> {
    store
        .list_items(ListItemsParams {
            thread_id,
            include_archived: false,
            turn_id: Some(turn_id.to_string()),
            item_ids: None,
            position: None,
            page_size: 20,
            sort_direction: SortDirection::Asc,
            sort_key: ItemSortKey::CreatedAtOrdinal,
            after_updated_at_ordinal: None,
            before_updated_at_ordinal: None,
        })
        .await
        .expect("list items")
        .items
        .into_iter()
        .map(|item| item.item_id)
        .collect()
}

async fn model_message_texts(store: &LocalThreadStore, thread_id: ThreadId) -> Vec<String> {
    store
        .load_latest_model_context(LoadThreadHistoryParams {
            thread_id,
            include_archived: false,
        })
        .await
        .expect("load model context")
        .items
        .into_iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => match envelope.item {
                ResponseItem::Message { content, .. } => {
                    content.into_iter().find_map(|part| match part {
                        ContentItem::InputText { text } | ContentItem::OutputText { text, .. } => {
                            Some(text)
                        }
                        ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => None,
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn user_response(text: &str) -> RolloutItem {
    RolloutItem::ResponseItem(
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            status: None,
            encrypted_content: None,
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
        .into(),
    )
}

fn assistant_response(text: &str) -> RolloutItem {
    RolloutItem::ResponseItem(
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                annotations: None,
                logprobs: None,
                text: text.to_string(),
            }],
            status: None,
            encrypted_content: None,
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
        .into(),
    )
}

fn user_item_completed(
    thread_id: ThreadId,
    turn_id: &str,
    item_id: &str,
    text: &str,
) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id,
        turn_id: turn_id.to_string(),
        item: TurnItem::UserMessage(UserMessageItem {
            id: item_id.to_string(),
            client_id: None,
            content: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
        }),
        started_at_ms: Some(10),
        completed_at_ms: 11,
        content_token_count: None,
    }))
}

fn agent_item_completed(
    thread_id: ThreadId,
    turn_id: &str,
    item_id: &str,
    text: &str,
) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id,
        turn_id: turn_id.to_string(),
        item: TurnItem::AgentMessage(AgentMessageItem {
            id: item_id.to_string(),
            content: vec![AgentMessageContent::Text {
                text: text.to_string(),
            }],
            phase: None,
            memory_citation: None,
            delivery: None,
            moderation: None,
            questions: None,
        }),
        started_at_ms: Some(12),
        completed_at_ms: 13,
        content_token_count: None,
    }))
}
