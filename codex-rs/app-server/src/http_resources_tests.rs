use super::*;
use anyhow::Result;
use axum::body::to_bytes;
use codex_analytics::AnalyticsEventsClient;
use codex_arg0::Arg0DispatchPaths;
use codex_login::AuthManager;
use codex_protocol::ContextWindowId;
use codex_protocol::ThreadId;
use codex_protocol::models::BaseInstructions;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_thread_store::CreateThreadParams;
use codex_thread_store::InMemoryThreadStore;
use codex_thread_store::ThreadMetadataPatch;
use codex_thread_store::ThreadPersistenceMetadata;
use codex_thread_store::ThreadStore;
use codex_thread_store::UpdateThreadMetadataParams;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::config_manager::ConfigManager;
use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessageSender;
use crate::request_processors::thread_list_state::ThreadListState;
use crate::skills_watcher::SkillsWatcher;
use crate::thread_sample::SampleResponseRegistry;
use crate::thread_state::ThreadStateManager;
use crate::thread_status::ThreadWatchManager;

const TINY_PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

async fn response_parts(response: Response) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body")
        .to_vec();
    (status, body)
}

async fn test_processor_with_store(
    test: &TestCodex,
    thread_store: Arc<dyn ThreadStore>,
) -> Result<(ThreadRequestProcessor, mpsc::Receiver<OutgoingEnvelope>)> {
    test_processor_with_state_db(test, thread_store, /*state_db*/ None).await
}

async fn test_processor_with_state_db(
    test: &TestCodex,
    thread_store: Arc<dyn ThreadStore>,
    state_db: Option<codex_core::StateDbHandle>,
) -> Result<(ThreadRequestProcessor, mpsc::Receiver<OutgoingEnvelope>)> {
    let config = Arc::new(test.config.clone());
    let auth =
        AuthManager::shared_from_config(config.as_ref(), /*enable_codex_api_key_env*/ false)
            .await?;
    let config_manager =
        ConfigManager::without_managed_config_for_tests(config.codex_home.to_path_buf());
    let (outgoing_tx, outgoing_rx) = mpsc::channel(/*buffer*/ 16);
    let outgoing = Arc::new(OutgoingMessageSender::new(
        outgoing_tx,
        AnalyticsEventsClient::disabled(),
    ));
    let thread_state_manager = ThreadStateManager::new();
    let thread_watch_manager = ThreadWatchManager::new_with_outgoing(outgoing.clone());
    let skills_watcher = SkillsWatcher::new(
        test.thread_manager.skills_service(),
        &config.codex_home,
        outgoing.clone(),
    );
    let processor = ThreadRequestProcessor::new(
        auth,
        test.thread_manager.clone(),
        outgoing,
        Arg0DispatchPaths::default(),
        config,
        config_manager,
        thread_store,
        Default::default(),
        thread_state_manager,
        thread_watch_manager,
        Arc::new(ThreadListState::default()),
        state_db,
        /*log_db*/ None,
        skills_watcher,
        /*turn_cost_worker*/ None,
        Vec::new(),
        Arc::new(SampleResponseRegistry::default()),
    );
    Ok((processor, outgoing_rx))
}

fn attachment_request(thread: &str, workspace: &str, path: &str, probe: bool) -> ResourceParams {
    ResourceParams {
        thread: thread.to_string(),
        startup: String::new(),
        subscription: String::new(),
        body: Some(serde_json::json!({
            "path": path,
            "workspace": workspace,
            "probe": probe,
        })),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn resolves_workspace_file_links_without_following_links_outside_the_workspace() {
    let temp = tempfile::tempdir().expect("create temp directory");
    let root = temp.path().join("workspace");
    let outside = temp.path().join("outside.txt");
    let linked = root.join("file.txt");
    std::fs::create_dir(&root).expect("create workspace");
    std::fs::write(&linked, "inside").expect("create workspace file");
    std::fs::write(&outside, "outside").expect("create separate file");
    std::os::unix::fs::symlink(&outside, root.join("escape.txt")).expect("link separate file");

    let canonical = linked.canonicalize().expect("resolve test file");
    assert_eq!(
        resolve_detail_file(&root, Path::new("file.txt")).await,
        Some(canonical.clone())
    );
    assert_eq!(resolve_detail_file(&root, &linked).await, Some(canonical));
    assert_eq!(
        resolve_detail_file(&root, Path::new("../outside.txt")).await,
        None
    );
    assert_eq!(
        resolve_detail_file(&root, Path::new("escape.txt")).await,
        None
    );
}

#[cfg(unix)]
#[tokio::test]
async fn attachment_probe_distinguishes_directories_from_named_pipes() {
    let temp = tempfile::tempdir().expect("create temp directory");
    let path = temp.path().join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("create named pipe")
            .success()
    );
    assert!(
        read_workspace_image_file(temp.path(), Path::new("."), ImageFileRead::Probe)
            .await
            .expect("directories are not images")
            .is_empty()
    );
    let error = read_workspace_image_file(temp.path(), Path::new("pipe"), ImageFileRead::Probe)
        .await
        .expect_err("named pipes cannot be read as image attachments");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_image_read_confines_paths_and_returns_uniform_errors() {
    let temp = tempfile::tempdir().expect("create temp directory");
    let root = temp.path().join("workspace");
    let sibling = temp.path().join("workspace-other");
    let sub = root.join("nested");
    std::fs::create_dir_all(&sub).expect("create workspace dirs");
    std::fs::create_dir(&sibling).expect("create sibling dir");

    let inside_png = sub.join("ok.png");
    let outside_png = temp.path().join("secret.png");
    let outside_txt = temp.path().join("secret.txt");
    let sibling_png = sibling.join("sibling.png");
    std::fs::write(&inside_png, TINY_PNG).expect("write inside png");
    std::fs::write(&outside_png, TINY_PNG).expect("write outside png");
    std::fs::write(&outside_txt, b"not an image").expect("write outside text");
    std::fs::write(&sibling_png, TINY_PNG).expect("write sibling png");

    std::os::unix::fs::symlink(&inside_png, root.join("inside-link.png")).expect("symlink inside");
    std::os::unix::fs::symlink(&outside_png, root.join("escape-png.png"))
        .expect("symlink outside png");
    std::os::unix::fs::symlink(&outside_txt, root.join("escape-txt.txt"))
        .expect("symlink outside txt");
    std::os::unix::fs::symlink(temp.path().join("does-not-exist"), root.join("dangling"))
        .expect("dangling symlink");
    let workspace_alias = temp.path().join("workspace-alias");
    std::os::unix::fs::symlink(&root, &workspace_alias).expect("symlink workspace alias");

    // Legitimate reads inside workspace succeed (relative, absolute, internal symlink, aliased root).
    for candidate in [
        PathBuf::from("nested/ok.png"),
        inside_png.clone(),
        PathBuf::from("inside-link.png"),
        workspace_alias.join("nested/ok.png"),
    ] {
        assert_eq!(
            read_workspace_image_file(&root, &candidate, ImageFileRead::Preview)
                .await
                .expect("inside workspace image should be readable"),
            TINY_PNG
        );
        assert_eq!(
            read_workspace_image_file(&workspace_alias, &candidate, ImageFileRead::Probe)
                .await
                .expect("aliased workspace probe should succeed"),
            TINY_PNG
        );
    }

    // Outside-workspace, traversal, sibling-prefix, symlink-escape, and missing paths all yield
    // the exact same 404 response in both probe and preview modes (no existence oracle).
    let baseline_probe = response_parts(
        read_workspace_image_file(&root, Path::new("missing.png"), ImageFileRead::Probe)
            .await
            .expect_err("missing inside path must fail"),
    )
    .await;
    let baseline_preview = response_parts(
        read_workspace_image_file(&root, Path::new("missing.png"), ImageFileRead::Preview)
            .await
            .expect_err("missing inside path must fail"),
    )
    .await;
    assert_eq!(baseline_probe.0, StatusCode::NOT_FOUND);
    assert_eq!(baseline_probe, baseline_preview);

    let rejected_candidates = [
        outside_png,
        outside_txt,
        temp.path().join("missing-outside.png"),
        sibling_png,
        PathBuf::from("../secret.png"),
        PathBuf::from("../secret.txt"),
        PathBuf::from("../missing-outside.png"),
        PathBuf::from("escape-png.png"),
        PathBuf::from("escape-txt.txt"),
        PathBuf::from("dangling"),
    ];
    for candidate in rejected_candidates {
        for mode in [ImageFileRead::Probe, ImageFileRead::Preview] {
            let err = read_workspace_image_file(&root, &candidate, mode)
                .await
                .expect_err("outside or escaping candidate must be rejected");
            assert_eq!(
                response_parts(err).await,
                baseline_probe,
                "response must be uniform for {}",
                candidate.display()
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn handle_attachment_image_confines_thread_workspace_without_existence_oracle() -> Result<()>
{
    let server = wiremock::MockServer::start().await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let thread_id = test.session_configured.thread_id.to_string();
    let workspace = test.cwd_path().to_path_buf();
    let (processor, _outgoing_rx) =
        test_processor_with_store(&test, test.thread_store.clone()).await?;

    let inside_png = workspace.join("inside.png");
    let inside_txt = workspace.join("notes.txt");
    std::fs::write(&inside_png, TINY_PNG)?;
    std::fs::write(&inside_txt, b"plain text")?;

    let outside_dir = tempfile::tempdir()?;
    let outside_png = outside_dir.path().join("host.png");
    let outside_txt = outside_dir.path().join("host.txt");
    let outside_missing = outside_dir.path().join("missing.txt");
    std::fs::write(&outside_png, TINY_PNG)?;
    std::fs::write(&outside_txt, b"host secret")?;
    std::os::unix::fs::symlink(&outside_png, workspace.join("link-outside.png"))?;

    // Valid image inside thread workspace returns 200 with raster bytes / detected mime.
    let (status, bytes) = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(&thread_id, "", "inside.png", /*probe*/ false),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, TINY_PNG);

    let (status, probe_body) = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(&thread_id, "", "inside.png", /*probe*/ true),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&probe_body)?,
        serde_json::json!({ "mime": "image/png" })
    );

    // Non-raster file inside workspace returns 415 on preview and null mime on probe.
    let (status, _) = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(&thread_id, "", "notes.txt", /*probe*/ false),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let (status, probe_body) = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(&thread_id, "", "notes.txt", /*probe*/ true),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&probe_body)?,
        serde_json::json!({ "mime": null })
    );

    // Outside paths (image, non-image, missing, symlink escape, traversal) must all return the
    // exact same 404 payload in both preview and probe modes.
    let baseline = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(
                &thread_id,
                "",
                outside_missing.to_string_lossy().as_ref(),
                /*probe*/ false,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(baseline.0, StatusCode::NOT_FOUND);

    let outside_paths = [
        outside_png.to_string_lossy().into_owned(),
        outside_txt.to_string_lossy().into_owned(),
        outside_missing.to_string_lossy().into_owned(),
        "link-outside.png".to_string(),
        "../outside-relative.png".to_string(),
    ];
    for path in outside_paths {
        for probe in [false, true] {
            let actual = response_parts(
                handle_attachment_image(
                    &processor,
                    attachment_request(&thread_id, "", &path, probe),
                )
                .await,
            )
            .await;
            assert_eq!(
                actual, baseline,
                "outside path `{path}` (probe={probe}) must not reveal file existence or type"
            );
        }
    }

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn handle_attachment_image_requires_admitted_workspace_when_thread_is_empty() -> Result<()> {
    let server = wiremock::MockServer::start().await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let live_workspace = test.cwd_path().to_path_buf();

    // Also register a persisted thread workspace in the store so empty-thread previews admit both
    // live and persisted thread workspaces.
    let stored_dir = tempfile::tempdir()?;
    let stored_workspace = stored_dir.path().join("stored-workspace");
    std::fs::create_dir(&stored_workspace)?;
    std::fs::write(stored_workspace.join("stored.png"), TINY_PNG)?;
    std::fs::write(live_workspace.join("live.png"), TINY_PNG)?;

    let store = Arc::new(InMemoryThreadStore::default());
    let stored_thread_id = ThreadId::new();
    store
        .create_thread(CreateThreadParams {
            attached_thread: false,
            creator_user_id: None,
            creator_account_id: None,
            session_id: stored_thread_id.into(),
            thread_id: stored_thread_id,
            extra_config: None,
            forked_from_id: None,
            parent_thread_id: None,
            source: SessionSource::Cli,
            thread_source: None,
            originator: "mekugi_test".to_string(),
            base_instructions: BaseInstructions::default(),
            dynamic_tools: Vec::new(),
            selected_capability_roots: Vec::new(),
            multi_agent_version: None,
            history_mode: ThreadHistoryMode::Legacy,
            history_base: None,
            subagent_history_start_ordinal: None,
            initial_window_id: ContextWindowId::new_uuid_v7(),
            runtime_workspace_roots: None,
            metadata: ThreadPersistenceMetadata {
                cwd: Some(stored_workspace.clone()),
                model_provider: "test-provider".to_string(),
                memory_mode: ThreadMemoryMode::Enabled,
            },
        })
        .await?;
    store
        .update_thread_metadata(UpdateThreadMetadataParams {
            thread_id: stored_thread_id,
            include_archived: false,
            patch: ThreadMetadataPatch {
                cwd: Some(stored_workspace.clone()),
                ..Default::default()
            },
        })
        .await?;

    let (processor, _outgoing_rx) = test_processor_with_store(&test, store).await?;

    // Unadmitted host directories (existing, root, missing, file, relative, traversal, external
    // symlink to an admitted workspace) must all fail with the exact same 400 response.
    let unadmitted_dir = tempfile::tempdir()?;
    let unadmitted_file = unadmitted_dir.path().join("file.png");
    std::fs::write(&unadmitted_file, TINY_PNG)?;
    let external_symlink = unadmitted_dir.path().join("alias-to-live");
    std::os::unix::fs::symlink(&live_workspace, &external_symlink)?;

    let missing_workspace = unadmitted_dir.path().join("does-not-exist");
    let baseline = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(
                "",
                missing_workspace.to_string_lossy().as_ref(),
                "file.png",
                /*probe*/ false,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(baseline.0, StatusCode::BAD_REQUEST);

    let unadmitted_workspaces = [
        unadmitted_dir.path().to_string_lossy().into_owned(),
        "/".to_string(),
        unadmitted_file.to_string_lossy().into_owned(),
        external_symlink.to_string_lossy().into_owned(),
        "relative/workspace".to_string(),
        format!("{}/..", live_workspace.display()),
        String::new(),
    ];
    for workspace in unadmitted_workspaces {
        for probe in [false, true] {
            let actual = response_parts(
                handle_attachment_image(
                    &processor,
                    attachment_request("", &workspace, "file.png", probe),
                )
                .await,
            )
            .await;
            assert_eq!(
                actual, baseline,
                "unadmitted workspace `{workspace}` (probe={probe}) must not probe host directories"
            );
        }
    }

    // Admitted live and persisted workspaces work for contained images and still confine paths.
    for (workspace, file_name) in [
        (&live_workspace, "live.png"),
        (&stored_workspace, "stored.png"),
    ] {
        let workspace_str = workspace.to_string_lossy();
        let (status, bytes) = response_parts(
            handle_attachment_image(
                &processor,
                attachment_request("", workspace_str.as_ref(), file_name, /*probe*/ false),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, TINY_PNG);

        let (status, _) = response_parts(
            handle_attachment_image(
                &processor,
                attachment_request(
                    "",
                    workspace_str.as_ref(),
                    unadmitted_file.to_string_lossy().as_ref(),
                    /*probe*/ false,
                ),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn open_confined_resource_file_reads_files_behind_search_only_directories() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    struct RestorePermissions(Vec<PathBuf>);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            for path in self.0.iter().rev() {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    let shared = workspace.join("shared");
    std::fs::create_dir_all(&shared)?;
    let image_path = shared.join("diagram.png");
    let text_path = shared.join("notes.txt");
    std::fs::write(&image_path, TINY_PNG)?;
    std::fs::write(&text_path, b"readable behind search-only dir")?;

    let _guard = RestorePermissions(vec![workspace.clone(), shared.clone()]);
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o111))?;
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o111))?;

    // When running non-root, directory listing must fail while search-only traversal succeeds.
    // SAFETY: `geteuid` has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert!(std::fs::read_dir(&shared).is_err());
        assert!(std::fs::read_dir(&workspace).is_err());
    }

    assert_eq!(
        read_workspace_image_file(
            &workspace,
            Path::new("shared/diagram.png"),
            ImageFileRead::Preview,
        )
        .await
        .expect("readable image behind search-only dirs should succeed"),
        TINY_PNG
    );

    let (mut file, metadata) =
        open_confined_resource_file(&workspace, Path::new("shared/notes.txt"))
            .await
            .expect("readable detail file behind search-only dirs should open");
    assert!(metadata.is_file());
    let mut text = String::new();
    file.read_to_string(&mut text).await?;
    assert_eq!(text, "readable behind search-only dir");

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn admit_attachment_workspace_uses_state_db_without_rollout_scan() -> Result<()> {
    let server = wiremock::MockServer::start().await;
    let test = test_codex().build_with_auto_env(&server).await?;

    let db_home = tempfile::tempdir()?;
    let state_db = codex_state::StateRuntime::init(
        codex_state::SqliteConfig::new_for_testing(
            codex_utils_absolute_path::AbsolutePathBuf::try_from(db_home.path())?,
        ),
        "test-provider".to_string(),
    )
    .await?;

    let db_workspace_dir = tempfile::tempdir()?;
    let db_workspace = db_workspace_dir.path().canonicalize()?;
    std::fs::write(db_workspace.join("saved.png"), TINY_PNG)?;

    let mut metadata = codex_state::ThreadMetadataBuilder::new(
        ThreadId::new(),
        db_home.path().join("saved-thread.jsonl"),
        chrono::Utc::now(),
        SessionSource::Cli,
    )
    .build("test-provider");
    metadata.cwd = db_workspace.clone();
    metadata.preview = Some("saved thread".to_string());
    state_db.upsert_thread(&metadata).await?;

    let store = Arc::new(InMemoryThreadStore::default());
    let (processor, _outgoing_rx) =
        test_processor_with_state_db(&test, store.clone(), Some(state_db)).await?;

    let (status, bytes) = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(
                "",
                db_workspace.to_string_lossy().as_ref(),
                "saved.png",
                /*probe*/ false,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, TINY_PNG);
    assert_eq!(
        store.calls().await.list_threads,
        0,
        "state_db lookup must not fall back to thread_store rollout listing"
    );

    let unadmitted = tempfile::tempdir()?;
    let (status, _) = response_parts(
        handle_attachment_image(
            &processor,
            attachment_request(
                "",
                unadmitted.path().to_string_lossy().as_ref(),
                "saved.png",
                /*probe*/ false,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        store.calls().await.list_threads,
        0,
        "unadmitted workspace rejection must not trigger rollout listing when state_db is active"
    );

    Ok(())
}

#[test]
fn detects_supported_raster_images_only() {
    assert_eq!(
        detect_raster_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
        Some("image/png")
    );
    assert_eq!(
        detect_raster_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
        Some("image/jpeg")
    );
    assert_eq!(detect_raster_mime(b"GIF89a..."), Some("image/gif"));
    assert_eq!(detect_raster_mime(b"RIFF0000WEBP"), Some("image/webp"));
    assert_eq!(detect_raster_mime(b"<svg></svg>"), None);
    assert_eq!(detect_raster_mime(b"plain text"), None);
}

#[test]
fn finds_published_user_and_generated_and_tool_images() {
    use codex_app_server_protocol::ImageGenerationItem;
    use codex_app_server_protocol::McpToolCallResult;
    use codex_app_server_protocol::McpToolCallStatus;
    use codex_app_server_protocol::ThreadItem;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus;

    let user_url = "data:image/png;base64,iVBORw0KGgo=".to_string();
    let gen_b64 = "iVBORw0KGgoAAAANSUhEUg==".to_string();
    let gen_url = format!("data:image/png;base64,{gen_b64}");
    let mcp_url = "data:image/jpeg;base64,/9j/4AAQ".to_string();

    let turn = Turn {
        id: "turn-1".to_string(),
        root_turn_id: None,
        items: vec![
            ThreadItem::untimed(ThreadItemPayload::UserMessage {
                id: "user-1".to_string(),
                client_id: None,
                content: vec![UserInput::Image {
                    image: ImageReference::Inline {
                        url: user_url.clone(),
                    },
                    detail: None,
                }],
            }),
            ThreadItem::untimed(ThreadItemPayload::ImageGeneration(ImageGenerationItem {
                id: "gen-1".to_string(),
                status: "completed".to_string(),
                revised_prompt: None,
                image: GeneratedImageRef::Inline { result: gen_b64 },
                transparent_background: None,
                failure: None,
                saved_path: None,
                input_images: Vec::new(),
                imagegen_request_id: None,
                generation_id: None,
            })),
            ThreadItem::untimed(ThreadItemPayload::McpToolCall {
                id: "mcp-1".to_string(),
                server: "srv".to_string(),
                environment_id: None,
                tool: "render".to_string(),
                status: McpToolCallStatus::Completed,
                arguments: serde_json::json!({}),
                app_context: None,
                mcp_app_resource_uri: None,
                mcp_app_ui: None,
                plugin_id: None,
                read_only_hint: None,
                result: Some(Box::new(McpToolCallResult {
                    content: vec![serde_json::json!({
                        "type": "image",
                        "mimeType": "image/jpeg",
                        "data": "/9j/4AAQ"
                    })],
                    structured_content: None,
                    meta: None,
                })),
                error: None,
                duration_ms: None,
            }),
        ],
        items_view: Default::default(),
        status: TurnStatus::Completed,
        error: None,
        started_at: None,
        completed_at: None,
        duration_ms: None,
    };

    assert_eq!(
        find_published_image_source(
            std::slice::from_ref(&turn),
            &ImageBody {
                item_id: Some("user-1".to_string()),
                index: Some(0),
                image: image_identity(&user_url),
            }
        ),
        Some(user_url)
    );
    assert_eq!(
        find_published_image_source(
            std::slice::from_ref(&turn),
            &ImageBody {
                item_id: Some("gen-1".to_string()),
                index: Some(0),
                image: image_identity(&gen_url),
            }
        ),
        Some(gen_url)
    );
    assert_eq!(
        find_published_image_source(
            std::slice::from_ref(&turn),
            &ImageBody {
                item_id: Some("mcp-1".to_string()),
                index: Some(0),
                image: image_identity(&mcp_url),
            }
        ),
        Some(mcp_url)
    );
}
