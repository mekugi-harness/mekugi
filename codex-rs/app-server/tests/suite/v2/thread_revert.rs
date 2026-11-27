use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::create_request_user_input_sse_response;
use codex_app_server_protocol::AskForApproval;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CommandExecutionSource;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SortDirection;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadHistoryMode;
use codex_app_server_protocol::ThreadItemPayload;
use codex_app_server_protocol::ThreadItemsListCursor;
use codex_app_server_protocol::ThreadItemsListParams;
use codex_app_server_protocol::ThreadItemsListResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadRevertParams;
use codex_app_server_protocol::ThreadRevertPreviewParams;
use codex_app_server_protocol::ThreadRevertPreviewResponse;
use codex_app_server_protocol::ThreadRevertResponse;
use codex_app_server_protocol::ThreadRevertedNotification;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadShellCommandParams;
use codex_app_server_protocol::ThreadShellCommandResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadTurnsListParams;
use codex_app_server_protocol::ThreadTurnsListResponse;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnItemsView;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_app_server_protocol::UserInput;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeId;
use codex_change_history::ChangeRecord;
use codex_change_history::ChangeSourceKind;
use codex_features::Feature;
use codex_protocol::ThreadId;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::MultiAgentVersion;
use codex_rollout::RolloutItem;
use codex_rollout::read_session_meta_line;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::test_support::PathExt;
use codex_utils_path_uri::LegacyAppPathString;
use core_test_support::responses;
use core_test_support::skip_if_remote;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[test_case::test_case(false; "keep_edits")]
#[test_case::test_case(true; "revert_edits")]
#[tokio::test]
async fn thread_revert_can_keep_or_undo_native_shell_captures(undo: bool) -> Result<()> {
    skip_if_remote!(Ok(()), "uses the app-server host shell and workspace");
    let provider = create_mock_responses_server_repeating_assistant("Saved answer").await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&provider.uri()).write(codex_home.path())?;
    let workspace = TempDir::new()?;
    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build()
        .await?;
    initialize_experimental(&mut app).await?;
    let ThreadStartResponse { thread, .. } = app
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                cwd: Some(workspace.path().display().to_string()),
                ..Default::default()
            },
        })
        .await?;
    let thread_id = thread.id;
    let _: ThreadSetNameResponse = app
        .request(|request_id| ClientRequest::ThreadSetName {
            request_id,
            params: ThreadSetNameParams {
                thread_id: thread_id.clone(),
                name: "Captured edit rewind".into(),
            },
        })
        .await?;
    let mut turns = Vec::new();
    let prior_output = workspace.path().join("keep-me.txt");
    let mut prior_shell_turn = String::new();
    for text in ["Keep this history", "Remove this history"] {
        let completed = app
            .start_turn_and_wait_for_completion(TurnStartParams {
                thread_id: thread_id.clone(),
                input: vec![UserInput::Text {
                    text: text.into(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
        turns.push(completed.turn.id);
        if turns.len() == 1 {
            let command = format!(
                "printf 'prior edit' > {}",
                shlex::try_quote(&prior_output.to_string_lossy())?
            );
            let (exit, turn) = run_rewind_shell(&mut app, &thread_id, command).await?;
            assert_eq!(exit, 0);
            prior_shell_turn = turn;
        }
    }
    let output = workspace.path().join("rewind-me.txt");
    let command = format!(
        "printf 'captured edit' > {}",
        shlex::try_quote(&output.to_string_lossy())?
    );
    let (exit, captured_turn) = run_rewind_shell(&mut app, &thread_id, command).await?;
    assert_eq!(exit, 0);
    assert_eq!(std::fs::read_to_string(&output)?, "captured edit");

    // Make the retained shell execution and the next user message fall inside
    // the same clock second without depending on how quickly the host runs.
    let history_db = codex_state::open_thread_history_db(
        &codex_state::SqliteConfig::new_for_testing(codex_home.path().abs()),
    )
    .await?;
    let second: i64 = sqlx::query_scalar(
        "SELECT MAX(completed_at_ms) / 1000 FROM thread_items WHERE turn_id = ?",
    )
    .bind(&prior_shell_turn)
    .fetch_one(&history_db)
    .await?;
    for (turn_id, offset) in [(&prior_shell_turn, 100), (&captured_turn, 400)] {
        let wrote = sqlx::query(
            "UPDATE thread_items SET started_at_ms = ?, completed_at_ms = ? WHERE turn_id = ?",
        )
        .bind(second * 1000 + offset)
        .bind(second * 1000 + offset + 100)
        .bind(turn_id)
        .execute(&history_db)
        .await?;
        assert!(wrote.rows_affected() > 0);
    }
    let wrote = sqlx::query(
        "UPDATE thread_items SET created_at_ms = ? WHERE (turn_id, item_id) IN
         (SELECT turn_id, first_user_item_id FROM thread_turns WHERE turn_id = ?)",
    )
    .bind(second * 1000 + 300)
    .bind(&turns[1])
    .execute(&history_db)
    .await?;
    assert_eq!(wrote.rows_affected(), 1);
    let wrote = sqlx::query("UPDATE thread_turns SET started_at = ? WHERE turn_id = ?")
        .bind(second)
        .bind(&turns[1])
        .execute(&history_db)
        .await?;
    assert_eq!(wrote.rows_affected(), 1);
    drop(history_db);

    let preview = ThreadRevertPreviewParams {
        thread_id: thread_id.clone(),
        before_turn_id: turns[1].clone(),
        before_item_id: None,
    };
    let selection: ThreadRevertPreviewResponse = app
        .request(|request_id| ClientRequest::ThreadRevertPreview {
            request_id,
            params: preview.clone(),
        })
        .await?;
    assert_eq!(selection.reason, None);
    assert_eq!(selection.change_ids.len(), 1);
    let capture_dir = codex_protocol::shell_environment::session_tmp_path(
        ThreadId::from_string(&thread_id)?.into(),
    )
    .join("change-history");
    let store = ChangeHistoryStore::open_existing(&capture_dir)?;
    let params = ThreadRevertParams {
        thread_id: thread_id.clone(),
        before_turn_id: turns[1].clone(),
        before_item_id: None,
        expected_change_ids: undo.then_some(selection.change_ids.clone()),
    };

    if undo {
        let mut stale = params.clone();
        stale.expected_change_ids = Some(vec!["shell999999".to_string()]);
        assert!(
            rewind_error(&mut app, &stale)
                .await?
                .error
                .message
                .contains("Captured edits changed")
        );
        assert_eq!(std::fs::read_to_string(&output)?, "captured edit");

        std::fs::write(&output, "unrelated unsaved change")?;
        assert!(
            rewind_error(&mut app, &params)
                .await?
                .error
                .message
                .contains("drift")
        );
        assert_eq!(
            std::fs::read_to_string(&output)?,
            "unrelated unsaved change"
        );
        std::fs::write(&output, "captured edit")?;

        let previous: ChangeId = selection.change_ids[0].parse()?;
        let blocked = capture_dir.join("records").join(format!(
            "{}.json",
            ChangeId::new_unattributed(previous.number() + 1)?
        ));
        std::fs::create_dir(&blocked)?;
        let failure = rewind_error(&mut app, &params).await?;
        assert!(
            failure
                .error
                .message
                .contains("change history storage failure"),
            "{failure:?}"
        );
        assert_eq!(std::fs::read_to_string(&output)?, "captured edit");
        std::fs::remove_dir(&blocked)?;

        let sqlite = codex_state::SqliteConfig::new_for_testing(codex_home.path().abs());
        let pool = sqlite.open_read_write_pool(&sqlite.state_db_path()).await?;
        sqlx::query(
            "CREATE TRIGGER fail_rewind_cutover BEFORE UPDATE OF rollout_path ON threads
             WHEN OLD.rollout_path <> NEW.rollout_path
             BEGIN SELECT RAISE(FAIL, 'rewind history cutover fixture'); END",
        )
        .execute(&pool)
        .await?;
        let failure = rewind_error(&mut app, &params).await?;
        assert!(
            failure
                .error
                .message
                .contains("rewind history cutover fixture"),
            "{failure:?}"
        );
        assert_eq!(std::fs::read_to_string(&output)?, "captured edit");
        assert_eq!(
            store
                .list_entries()?
                .iter()
                .filter(|entry| { entry.source_kind == ChangeSourceKind::MchangesRevert })
                .count(),
            2,
            "undo and its compensation must both have run"
        );
        sqlx::query("DROP TRIGGER fail_rewind_cutover")
            .execute(&pool)
            .await?;
    }

    let _: ThreadRevertResponse = app
        .request(|request_id| ClientRequest::ThreadRevert { request_id, params })
        .await?;
    let kept: ThreadTurnsListResponse = app
        .request(|request_id| ClientRequest::ThreadTurnsList {
            request_id,
            params: ThreadTurnsListParams {
                thread_id: thread_id.clone(),
                cursor: None,
                limit: Some(10),
                sort_direction: Some(SortDirection::Asc),
                items_view: Some(TurnItemsView::Summary),
            },
        })
        .await?;
    assert_eq!(
        kept.data
            .into_iter()
            .map(|turn| turn.id)
            .collect::<Vec<_>>(),
        vec![turns.remove(0), prior_shell_turn],
    );
    assert_eq!(std::fs::read_to_string(prior_output)?, "prior edit");
    if undo {
        assert!(
            !output.exists(),
            "Undo did not remove the captured creation"
        );
    } else {
        assert_eq!(std::fs::read_to_string(&output)?, "captured edit");

        // Keep removed the execution but not the file or its capture. Place the next
        // message in the same second to exercise the orphaned capture in preview.
        let ids = [selection.change_ids[0].parse()?];
        let [mut earlier]: [ChangeRecord; 1] = store.load_records(&ids)?.try_into().unwrap();
        assert!(earlier.recorded_at_ms.is_some());
        let second = earlier.metadata.created_at * 1000;
        earlier.recorded_at_ms = Some(second + 100);
        std::fs::write(
            capture_dir
                .join("records")
                .join(format!("{}.json", earlier.id)),
            serde_json::to_vec(&earlier)?,
        )?;

        let next = app
            .start_turn_and_wait_for_completion(TurnStartParams {
                thread_id: thread_id.clone(),
                input: vec![UserInput::Text {
                    text: "A new message after Keep".into(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
        let command = format!(
            "printf 'follow-up edit' > {}",
            shlex::try_quote(&output.to_string_lossy())?
        );
        let (exit, shell_turn) = run_rewind_shell(&mut app, &thread_id, command).await?;
        assert_eq!(exit, 0);
        let history_db = codex_state::open_thread_history_db(
            &codex_state::SqliteConfig::new_for_testing(codex_home.path().abs()),
        )
        .await?;
        sqlx::query("UPDATE thread_items SET created_at_ms = ? WHERE turn_id = ?")
            .bind(second + 500)
            .bind(&next.turn.id)
            .execute(&history_db)
            .await?;
        sqlx::query("UPDATE thread_items SET started_at_ms = ? WHERE turn_id = ?")
            .bind(second + 600)
            .bind(shell_turn)
            .execute(&history_db)
            .await?;
        let follow_up: ThreadRevertPreviewResponse = app
            .request(|request_id| ClientRequest::ThreadRevertPreview {
                request_id,
                params: ThreadRevertPreviewParams {
                    thread_id: thread_id.clone(),
                    before_turn_id: next.turn.id.clone(),
                    before_item_id: None,
                },
            })
            .await?;
        assert_eq!(follow_up.reason, None);
        assert_eq!(follow_up.change_ids.len(), 1);
        let _: ThreadRevertResponse = app
            .request(|request_id| ClientRequest::ThreadRevert {
                request_id,
                params: ThreadRevertParams {
                    thread_id,
                    before_turn_id: next.turn.id,
                    before_item_id: None,
                    expected_change_ids: Some(follow_up.change_ids),
                },
            })
            .await?;
        assert_eq!(std::fs::read_to_string(output)?, "captured edit");
    }
    Ok(())
}

async fn rewind_error(
    app: &mut TestAppServer,
    params: &ThreadRevertParams,
) -> Result<JSONRPCError> {
    let id = app
        .send_request("thread/revert", Some(serde_json::to_value(params)?))
        .await?;
    Ok(timeout(
        DEFAULT_READ_TIMEOUT,
        app.read_stream_until_error_message(RequestId::Integer(id)),
    )
    .await??)
}

async fn run_rewind_shell(
    app: &mut TestAppServer,
    thread_id: &str,
    command: String,
) -> Result<(i32, String)> {
    let _: ThreadShellCommandResponse = app
        .request(|request_id| ClientRequest::ThreadShellCommand {
            request_id,
            params: ThreadShellCommandParams {
                thread_id: thread_id.to_string(),
                command: command.clone(),
                timeout_ms: None,
            },
        })
        .await?;
    let completed = timeout(
        DEFAULT_READ_TIMEOUT,
        app.read_stream_until_matching_notification("completed user shell command", |event| {
            event.method == "item/completed"
                && event.params.as_ref().is_some_and(|params| {
                    params["threadId"] == thread_id
                        && params["item"]["type"] == "commandExecution"
                        && params["item"]["source"] == "userShell"
                        && params["item"]["command"]
                            .as_str()
                            .is_some_and(|observed| observed.contains(&command))
                })
        }),
    )
    .await??;
    let completed: ItemCompletedNotification =
        serde_json::from_value(completed.params.expect("matched completion has parameters"))?;
    let ThreadItemPayload::CommandExecution {
        source: CommandExecutionSource::UserShell,
        exit_code: Some(exit_code),
        ..
    } = completed.item.payload
    else {
        anyhow::bail!("expected completed user shell item");
    };
    timeout(
        DEFAULT_READ_TIMEOUT,
        app.read_stream_until_matching_notification("user shell turn settled", |event| {
            event.method == "turn/completed"
                && event.params.as_ref().is_some_and(|params| {
                    params["threadId"] == thread_id && params["turn"]["id"] == completed.turn_id
                })
        }),
    )
    .await??;
    Ok((exit_code, completed.turn_id))
}

#[test_case::test_case(None; "default_paginated")]
#[test_case::test_case(Some(ThreadHistoryMode::Legacy); "explicit_legacy")]
#[tokio::test]
async fn thread_revert_rejects_ephemeral_without_losing_context(
    history_mode: Option<ThreadHistoryMode>,
) -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Remembered answer").await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri()).write(codex_home.path())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build()
        .await?;
    initialize_experimental(&mut mcp).await?;
    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            ephemeral: Some(true),
            history_mode,
            ..Default::default()
        })
        .await?;
    let completed = mcp
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "Remember this task".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let request_id = mcp
        .send_request(
            "thread/revert",
            Some(serde_json::to_value(ThreadRevertParams {
                thread_id: thread.id.clone(),
                before_turn_id: completed.turn.id,
                before_item_id: None,
                expected_change_ids: None,
            })?),
        )
        .await?;
    let error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(
        error.error,
        codex_app_server_protocol::JSONRPCErrorError {
            code: -32600,
            message: "ephemeral threads do not support thread/revert".to_string(),
            data: None,
        }
    );
    mcp.start_turn_and_wait_for_completion(TurnStartParams {
        thread_id: thread.id,
        input: vec![UserInput::Text {
            text: "Continue".to_string(),
            text_elements: Vec::new(),
        }],
        ..Default::default()
    })
    .await?;
    let requests = server.received_requests().await.expect("response requests");
    let followup = requests
        .iter()
        .rev()
        .find(|request| request.url.path().ends_with("/responses"))
        .expect("followup request")
        .body_json::<Value>()?;
    let input = followup["input"].as_array().expect("model input");
    for (role, text) in [
        ("user", "Remember this task"),
        ("assistant", "Remembered answer"),
    ] {
        assert!(
            input.iter().any(|item| item["role"] == role
                && item["content"]
                    .as_array()
                    .is_some_and(|content| content.iter().any(|part| part["text"] == text))),
            "missing prior {role} message from followup: {followup}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn thread_revert_preserves_fork_cutoff_after_cold_resume() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    let updated_workspace = TempDir::new()?;
    let saved_cwd = AbsolutePathBuf::from_absolute_path(updated_workspace.path().canonicalize()?)?
        .into_path_buf();
    let extra_workspace = TempDir::new()?;
    let saved_roots = vec![
        AbsolutePathBuf::from_absolute_path(&saved_cwd)?,
        AbsolutePathBuf::from_absolute_path(extra_workspace.path().canonicalize()?)?,
    ];
    MockResponsesConfig::new(&server.uri()).write(codex_home.path())?;
    // This fixture checks host-native cwd and workspace restoration across fork and revert.
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build()
        .await?;
    initialize_experimental(&mut mcp).await?;
    let ThreadStartResponse { thread: parent, .. } = mcp
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                history_mode: Some(ThreadHistoryMode::Paginated),
                ..Default::default()
            },
        })
        .await?;
    let mut parent_turns = Vec::new();
    for text in ["parent first", "parent second"] {
        let completed = mcp
            .start_turn_and_wait_for_completion(TurnStartParams {
                thread_id: parent.id.clone(),
                cwd: Some(parent.cwd.as_path().to_path_buf()),
                input: vec![UserInput::Text {
                    text: text.to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
        parent_turns.push(completed.turn.id);
    }
    let ThreadForkResponse { thread: child, .. } = mcp
        .request(|request_id| ClientRequest::ThreadFork {
            request_id,
            params: ThreadForkParams {
                thread_id: parent.id.clone(),
                cwd: Some(codex_home.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        })
        .await?;
    let child_meta = read_session_meta_line(child.path.as_ref().expect("child rollout"))
        .await?
        .meta;
    let fork_cutoff = child_meta
        .history_base
        .expect("fork history base")
        .end_ordinal_exclusive;
    assert_eq!(child_meta.forked_from_ordinal_exclusive, Some(fork_cutoff));
    let inherited_revert_cutoff =
        std::fs::read_to_string(parent.path.as_ref().expect("parent rollout"))?
            .lines()
            .map(codex_rollout::parse_rollout_line)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find_map(|line| match line.item {
                RolloutItem::EventMsg(EventMsg::TurnStarted(turn))
                    if turn.turn_id == parent_turns[1] =>
                {
                    line.ordinal
                }
                _ => None,
            })
            .expect("inherited turn start ordinal");
    let mut child_turns = Vec::new();
    for (text, runtime_workspace_roots) in [
        ("child first", None),
        ("child second", Some(saved_roots.clone())),
    ] {
        let completed = mcp
            .start_turn_and_wait_for_completion(TurnStartParams {
                thread_id: child.id.clone(),
                cwd: Some(saved_cwd.clone()),
                runtime_workspace_roots,
                input: vec![UserInput::Text {
                    text: text.to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
        child_turns.push(completed.turn.id);
    }

    // First revert within the child, then revert into its inherited parent history.
    for (before_turn_id, expected_cutoff) in [
        (child_turns[1].clone(), fork_cutoff),
        (parent_turns[1].clone(), inherited_revert_cutoff),
    ] {
        let ThreadRevertResponse {
            thread: reverted, ..
        } = mcp
            .request(|request_id| ClientRequest::ThreadRevert {
                request_id,
                params: ThreadRevertParams {
                    thread_id: child.id.clone(),
                    before_turn_id,
                    before_item_id: None,
                    expected_change_ids: None,
                },
            })
            .await?;
        let meta = read_session_meta_line(reverted.path.as_ref().expect("reverted rollout"))
            .await?
            .meta;
        assert_eq!(meta.forked_from_ordinal_exclusive, Some(expected_cutoff));
        if expected_cutoff == fork_cutoff {
            assert!(
                meta.history_base
                    .expect("child revert base")
                    .end_ordinal_exclusive
                    > fork_cutoff
            );
        }

        mcp.shutdown_gracefully().await?;
        mcp = TestAppServer::builder()
            .with_codex_home(codex_home.path())
            .without_auto_env()
            .build()
            .await?;
        initialize_experimental(&mut mcp).await?;
        let ThreadResumeResponse {
            cwd,
            runtime_workspace_roots,
            ..
        } = mcp
            .request(|request_id| ClientRequest::ThreadResume {
                request_id,
                params: ThreadResumeParams {
                    thread_id: child.id.clone(),
                    ..Default::default()
                },
            })
            .await?;
        assert_eq!(
            (cwd.as_path(), runtime_workspace_roots),
            (
                saved_cwd.as_path(),
                LegacyAppPathString::from_abs_paths(&saved_roots)
            )
        );
        mcp.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: child.id.clone(),
            input: vec![UserInput::Text {
                text: "continue after revert and cold resume".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
        let requests = server.received_requests().await.expect("response requests");
        let body = requests
            .iter()
            .rev()
            .find(|request| request.url.path().ends_with("/responses"))
            .expect("resumed model request")
            .body_json::<Value>()?;
        let metadata: Value = serde_json::from_str(
            body["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .expect("turn metadata"),
        )?;
        assert_eq!(
            (
                metadata["forked_from_thread_id"].as_str(),
                metadata["forked_from_ordinal_exclusive"].as_u64()
            ),
            (Some(parent.id.as_str()), Some(expected_cutoff))
        );
    }
    Ok(())
}

#[tokio::test]
async fn thread_revert_replaces_paginated_history_before_turn() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_mock =
        responses::mount_sse_sequence(&server, vec![responses::sse_completed("done"); 3]).await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .with_model("gpt-5.5")
        .enable_feature(Feature::FastMode)
        .write(codex_home.path())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build()
        .await?;
    initialize_experimental(&mut mcp).await?;

    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            history_mode: Some(ThreadHistoryMode::Paginated),
            service_tier: Some(Some("priority".to_string())),
            ..Default::default()
        })
        .await?;
    let stale_rollout_path = thread.path.clone().expect("thread rollout path");
    let mut turn_ids = Vec::new();
    for text in ["first", "second"] {
        let completed = mcp
            .start_turn_and_wait_for_completion(TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: text.to_string(),
                    text_elements: Vec::new(),
                }],
                model: (text == "second").then_some("gpt-daybreak-blue-latest".to_string()),
                ..Default::default()
            })
            .await?;
        turn_ids.push(completed.turn.id);
    }

    let ThreadRevertResponse {
        thread: reverted_thread,
        turns_backwards_cursor,
        items_backwards_cursor,
    } = mcp
        .request(|request_id| ClientRequest::ThreadRevert {
            request_id,
            params: ThreadRevertParams {
                thread_id: thread.id.clone(),
                before_turn_id: turn_ids[1].clone(),
                before_item_id: None,
                expected_change_ids: None,
            },
        })
        .await?;
    let reverted: ThreadRevertedNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_notification("thread/reverted"),
    )
    .await??;
    assert_eq!(reverted.thread_id, thread.id);

    assert_eq!(reverted_thread.id, thread.id);
    let replacement_path = reverted_thread.path.as_ref().expect("replacement rollout");
    assert_eq!(
        read_session_meta_line(replacement_path.as_path())
            .await?
            .meta
            .multi_agent_version,
        Some(MultiAgentVersion::V2)
    );
    assert!(reverted_thread.turns.is_empty());
    assert!(items_backwards_cursor.is_some());
    assert_eq!(
        turn_ids_from_cursor(
            &mut mcp,
            &thread.id,
            turns_backwards_cursor,
            /*sort_direction*/ None,
        )
        .await?,
        turn_ids[..1]
    );
    let ThreadItemsListResponse {
        data: reverted_items,
        ..
    } = mcp
        .request(|request_id| ClientRequest::ThreadItemsList {
            request_id,
            params: ThreadItemsListParams {
                thread_id: thread.id.clone(),
                turn_id: None,
                cursor: items_backwards_cursor.map(ThreadItemsListCursor::Opaque),
                limit: None,
                sort_direction: None,
                defer_output: None,
            },
        })
        .await?;
    assert!(!reverted_items.is_empty());
    assert!(
        reverted_items
            .iter()
            .all(|item| item.turn_id == turn_ids[0])
    );

    mcp.shutdown_gracefully().await?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build()
        .await?;
    initialize_experimental(&mut mcp).await?;
    let stale_resume_id = mcp
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            path: Some(stale_rollout_path),
            ..Default::default()
        })
        .await?;
    let stale_resume_error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(stale_resume_id)),
    )
    .await??;
    assert!(
        stale_resume_error.error.message.contains("stale path")
            && stale_resume_error
                .error
                .message
                .contains("omit path and resume by thread id"),
        "unexpected resume error: {}",
        stale_resume_error.error.message,
    );
    let resume_id = mcp
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let _: ThreadResumeResponse =
        timeout(DEFAULT_READ_TIMEOUT, mcp.read_response(resume_id)).await??;
    let invalid_revert_id = mcp
        .send_raw_request(
            "thread/revert",
            Some(serde_json::to_value(ThreadRevertParams {
                thread_id: thread.id.clone(),
                before_turn_id: "missing-turn".to_string(),
                before_item_id: None,
                expected_change_ids: None,
            })?),
        )
        .await?;
    let invalid_revert_error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(invalid_revert_id)),
    )
    .await??;
    assert_eq!(
        invalid_revert_error.error.message,
        "turn not found: missing-turn"
    );

    let third_turn = mcp
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "third".to_string(),
                text_elements: Vec::new(),
            }],
            model: Some("gpt-5.5".to_string()),
            ..Default::default()
        })
        .await?;
    let body = response_mock
        .last_request()
        .expect("third turn request")
        .body_json();
    assert_eq!(body["service_tier"], "priority");
    let model_input = body["input"].clone();
    let model_input = serde_json::to_string(&model_input)?;
    assert!(model_input.contains(r#""text":"first""#));
    assert!(!model_input.contains(r#""text":"second""#));
    assert!(model_input.contains(r#""text":"third""#));
    assert_eq!(
        turn_ids_from_cursor(
            &mut mcp,
            &thread.id,
            /*cursor*/ None,
            Some(SortDirection::Asc),
        )
        .await?,
        vec![turn_ids[0].clone(), third_turn.turn.id]
    );
    Ok(())
}

#[tokio::test]
async fn thread_revert_interrupts_active_turn_and_keeps_thread_loaded() -> Result<()> {
    let home = TempDir::new()?;
    let server = create_mock_responses_server_sequence(vec![
        create_final_assistant_message_sse_response("first")?,
        create_request_user_input_sse_response("call_blocked")?,
        create_final_assistant_message_sse_response("third")?,
    ])
    .await;
    MockResponsesConfig::new(&server.uri()).write(home.path())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(home.path())
        .build()
        .await?;
    initialize_experimental(&mut mcp).await?;

    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            history_mode: Some(ThreadHistoryMode::Paginated),
            ..Default::default()
        })
        .await?;
    let first_turn = mcp
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "first".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;

    let TurnStartResponse { turn: active_turn } = mcp
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: "sleep".to_string(),
                    text_elements: Vec::new(),
                }],
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Plan,
                    settings: Settings {
                        model: "mock-model".to_string(),
                        reasoning_effort: Some(ReasoningEffort::Medium),
                        developer_instructions: None,
                    },
                }),
                approval_policy: Some(AskForApproval::Never),
                ..Default::default()
            },
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_request_message(),
    )
    .await??;

    let ThreadRevertResponse {
        thread: reverted_thread,
        turns_backwards_cursor,
        items_backwards_cursor,
    } = mcp
        .request(|request_id| ClientRequest::ThreadRevert {
            request_id,
            params: ThreadRevertParams {
                thread_id: thread.id.clone(),
                before_turn_id: active_turn.id.clone(),
                before_item_id: None,
                expected_change_ids: None,
            },
        })
        .await?;
    let completed: TurnCompletedNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_notification("turn/completed"),
    )
    .await??;
    assert_eq!(completed.thread_id, thread.id);
    assert_eq!(completed.turn.status, TurnStatus::Interrupted);
    assert!(reverted_thread.turns.is_empty());
    assert!(items_backwards_cursor.is_some());
    assert_eq!(
        turn_ids_from_cursor(
            &mut mcp,
            &thread.id,
            turns_backwards_cursor,
            /*sort_direction*/ None,
        )
        .await?,
        vec![first_turn.turn.id]
    );

    let resumed: ThreadResumeResponse = mcp
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread.id.clone(),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.approval_policy, AskForApproval::Never);

    mcp.start_turn_and_wait_for_completion(TurnStartParams {
        thread_id: thread.id,
        input: vec![UserInput::Text {
            text: "third".to_string(),
            text_elements: Vec::new(),
        }],
        ..Default::default()
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn thread_revert_before_steered_user_message_preserves_prefix_and_model_context() -> Result<()>
{
    let home = TempDir::new()?;
    let server = create_mock_responses_server_sequence(vec![
        create_request_user_input_sse_response("call_pause")?,
        create_final_assistant_message_sse_response("Steered reply")?,
        create_final_assistant_message_sse_response("Post-revert reply")?,
    ])
    .await;
    MockResponsesConfig::new(&server.uri()).write(home.path())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(home.path())
        .build()
        .await?;
    initialize_experimental(&mut mcp).await?;

    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            history_mode: Some(ThreadHistoryMode::Paginated),
            ..Default::default()
        })
        .await?;

    let _: ThreadSetNameResponse = mcp
        .request(|request_id| ClientRequest::ThreadSetName {
            request_id,
            params: ThreadSetNameParams {
                thread_id: thread.id.clone(),
                name: "Rewind steered history".into(),
            },
        })
        .await?;

    let TurnStartResponse { turn: active_turn } = mcp
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: "Manage your own work".to_string(),
                    text_elements: Vec::new(),
                }],
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Plan,
                    settings: Settings {
                        model: "mock-model".to_string(),
                        reasoning_effort: Some(ReasoningEffort::Medium),
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let request = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_request_message(),
    )
    .await??;

    let steer: TurnSteerResponse = mcp
        .request(|request_id| ClientRequest::TurnSteer {
            request_id,
            params: TurnSteerParams {
                thread_id: thread.id.clone(),
                client_user_message_id: None,
                input: vec![UserInput::Text {
                    text: "Eliminate it then".to_string(),
                    text_elements: Vec::new(),
                }],
                responsesapi_client_metadata: None,
                responsesapi_client_metadata_patch: None,
                additional_context: None,
                expected_turn_id: active_turn.id.clone(),
            },
        })
        .await?;
    assert_eq!(steer.turn_id, active_turn.id);

    mcp.send_response(
        request.id().clone(),
        serde_json::json!({
            "answers": {
                "confirm_path": { "answers": ["Yes (Recommended)"] }
            }
        }),
    )
    .await?;
    let completed: TurnCompletedNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_notification("turn/completed"),
    )
    .await??;
    assert_eq!(completed.turn.id, active_turn.id);
    assert_eq!(completed.turn.status, TurnStatus::Completed);

    let ThreadTurnsListResponse { data: turns, .. } = mcp
        .request(|request_id| ClientRequest::ThreadTurnsList {
            request_id,
            params: ThreadTurnsListParams {
                thread_id: thread.id.clone(),
                cursor: None,
                limit: None,
                sort_direction: Some(SortDirection::Asc),
                items_view: Some(TurnItemsView::Full),
            },
        })
        .await?;
    assert_eq!(turns.len(), 1);
    let user_messages: Vec<(String, String)> = turns[0]
        .items
        .iter()
        .filter_map(|item| match &item.payload {
            ThreadItemPayload::UserMessage { content, .. } => {
                let text = content.iter().find_map(|part| match part {
                    UserInput::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })?;
                Some((item.id().to_string(), text))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        user_messages
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["Manage your own work", "Eliminate it then"]
    );
    let steer_item_id = user_messages[1].0.clone();

    // Empty beforeItemId is rejected without changing thread history.
    let bad_req = mcp
        .send_raw_request(
            "thread/revert",
            Some(serde_json::to_value(ThreadRevertParams {
                thread_id: thread.id.clone(),
                before_turn_id: active_turn.id.clone(),
                before_item_id: Some(String::new()),
                expected_change_ids: None,
            })?),
        )
        .await?;
    let bad_err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(bad_req)),
    )
    .await??;
    assert_eq!(bad_err.error.message, "beforeItemId must not be empty");

    // Mid-turn revert before the steered user message keeps the earlier portion of the turn.
    let ThreadRevertResponse {
        thread: reverted_thread,
        ..
    } = mcp
        .request(|request_id| ClientRequest::ThreadRevert {
            request_id,
            params: ThreadRevertParams {
                thread_id: thread.id.clone(),
                before_turn_id: active_turn.id.clone(),
                before_item_id: Some(steer_item_id),
                expected_change_ids: None,
            },
        })
        .await?;
    let reverted: ThreadRevertedNotification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_notification("thread/reverted"),
    )
    .await??;
    assert_eq!(reverted.thread_id, thread.id);
    assert_eq!(reverted_thread.id, thread.id);

    let ThreadTurnsListResponse {
        data: after_turns, ..
    } = mcp
        .request(|request_id| ClientRequest::ThreadTurnsList {
            request_id,
            params: ThreadTurnsListParams {
                thread_id: thread.id.clone(),
                cursor: None,
                limit: None,
                sort_direction: Some(SortDirection::Asc),
                items_view: Some(TurnItemsView::Full),
            },
        })
        .await?;
    assert_eq!(after_turns.len(), 1);
    assert_eq!(after_turns[0].id, active_turn.id);
    assert_eq!(after_turns[0].status, TurnStatus::Interrupted);
    let remaining_users: Vec<String> = after_turns[0]
        .items
        .iter()
        .filter_map(|item| match &item.payload {
            ThreadItemPayload::UserMessage { content, .. } => {
                content.iter().find_map(|part| match part {
                    UserInput::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect();
    assert_eq!(remaining_users, vec!["Manage your own work".to_string()]);

    // A subsequent turn must not send the reverted steer or its reply to the model.
    mcp.start_turn_and_wait_for_completion(TurnStartParams {
        thread_id: thread.id.clone(),
        input: vec![UserInput::Text {
            text: "Continue without steer".to_string(),
            text_elements: Vec::new(),
        }],
        ..Default::default()
    })
    .await?;

    let recorded = server
        .received_requests()
        .await
        .expect("mock server records requests");
    assert_eq!(recorded.len(), 3);
    let final_body = String::from_utf8_lossy(&recorded[2].body);
    assert!(
        final_body.contains("Manage your own work"),
        "expected retained initial prompt in model context: {final_body}"
    );
    assert!(
        final_body.contains("Continue without steer"),
        "expected new prompt in model context: {final_body}"
    );
    assert!(
        !final_body.contains("Eliminate it then"),
        "reverted steer leaked into model context: {final_body}"
    );
    assert!(
        !final_body.contains("Steered reply"),
        "reverted reply leaked into model context: {final_body}"
    );
    Ok(())
}

async fn turn_ids_from_cursor(
    mcp: &mut TestAppServer,
    thread_id: &str,
    cursor: Option<String>,
    sort_direction: Option<SortDirection>,
) -> Result<Vec<String>> {
    let ThreadTurnsListResponse { data, .. } = mcp
        .request(|request_id| ClientRequest::ThreadTurnsList {
            request_id,
            params: ThreadTurnsListParams {
                thread_id: thread_id.to_string(),
                cursor,
                limit: None,
                sort_direction,
                items_view: None,
            },
        })
        .await?;
    Ok(data.into_iter().map(|turn| turn.id).collect())
}

async fn initialize_experimental(mcp: &mut TestAppServer) -> Result<()> {
    let initialized = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.initialize_with_capabilities(
            ClientInfo {
                name: "test-client".to_string(),
                title: None,
                version: "0.1.0".to_string(),
            },
            Some(InitializeCapabilities {
                required_human_approval: false,
                explicit_gateway_oauth: false,
                experimental_api: true,
                request_attestation: false,
                opt_out_notification_methods: None,
                mcp_server_openai_form_elicitation: false,
                extensions: None,
            }),
        ),
    )
    .await??;
    assert!(matches!(initialized, JSONRPCMessage::Response(_)));
    Ok(())
}
