use std::fs;
use std::sync::Arc;
use std::time::Duration;

use codex_protocol::ThreadId;
use codex_utils_output_truncation::TruncationPolicy;
use core_test_support::skip_if_sandbox;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::sandboxing::SandboxPermissions;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context_with_auth_and_config_and_rx;
use crate::session::turn_context::TurnContext;
use crate::shell::ShellInvocation;
use crate::shell::get_shell_by_model_provided_path;
use crate::tools::context::ExecCommandToolOutput;
use crate::tools::handlers::EffectiveAdditionalPermissions;
use crate::tools::handlers::unified_exec::ResolvedCommand;
use crate::tools::handlers::unified_exec::preparation::PreparedExecCommand;
use crate::unified_exec::ExecCommandRequest;
use crate::unified_exec::MIN_EMPTY_YIELD_TIME_MS;
use crate::unified_exec::UnifiedExecContext;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcessManager;
use crate::unified_exec::WriteStdinRequest;
use crate::unified_exec::retained::retained_exec_thread_dir;

async fn test_session_and_turn() -> (Arc<Session>, Arc<TurnContext>) {
    let (session, _turn, _events) = make_session_and_context_with_auth_and_config_and_rx(
        codex_login::CodexAuth::from_api_key("Test API Key"),
        Vec::new(),
        |_config| {},
    )
    .await;
    let (turn, _) = session
        .new_turn_with_sub_id(
            "retained-exec-test".to_string(),
            crate::session::session::SessionSettingsUpdate {
                sandbox_policy: Some(codex_protocol::protocol::SandboxPolicy::DangerFullAccess),
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .expect("create DangerFullAccess turn");
    (session, turn)
}

fn make_context(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    call_id: &str,
) -> UnifiedExecContext {
    UnifiedExecContext::new(
        Arc::clone(session),
        StepContext::for_test(Arc::clone(turn)),
        CancellationToken::new(),
        call_id.to_string(),
    )
}

fn prepared_bash_command(
    turn: &TurnContext,
    cmd: &str,
    yield_time_ms: u64,
    max_output_tokens: Option<usize>,
    completion_timeout: Option<Duration>,
) -> PreparedExecCommand {
    let turn_environment = turn
        .initial_environments
        .primary()
        .expect("primary turn environment")
        .clone();
    #[allow(deprecated)]
    let cwd = turn.cwd.clone().into();
    let shell = get_shell_by_model_provided_path(&std::path::PathBuf::from("/bin/bash"));
    PreparedExecCommand {
        turn_environment,
        resolved: ResolvedCommand {
            command: vec!["bash".to_string(), "-lc".to_string(), cmd.to_string()],
            shell: ShellInvocation {
                shell,
                use_login_shell: false,
            },
        },
        hook_command: cmd.to_string(),
        cwd,
        permissions: EffectiveAdditionalPermissions {
            sandbox_permissions: SandboxPermissions::UseDefault,
            additional_permissions: None,
            permissions_preapproved: false,
        },
        justification: None,
        prefix_rule: None,
        tty: false,
        yield_time_ms,
        max_output_tokens,
        completion_timeout,
    }
}

async fn run_exec(
    manager: &UnifiedExecProcessManager,
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    call_id: &str,
    cmd: &str,
    yield_time_ms: u64,
    max_output_tokens: Option<usize>,
) -> Result<ExecCommandToolOutput, UnifiedExecError> {
    let process_id = manager.allocate_process_id().await?;
    let prepared = prepared_bash_command(turn, cmd, yield_time_ms, max_output_tokens, None);
    let context = make_context(session, turn, call_id);
    manager
        .exec_command(
            ExecCommandRequest {
                prepared,
                process_id,
                network: None,
            },
            &context,
        )
        .await
}

async fn run_write_stdin(
    manager: &UnifiedExecProcessManager,
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    call_id: &str,
    process_id: i32,
    input: &str,
    max_output_tokens: Option<usize>,
) -> Result<ExecCommandToolOutput, UnifiedExecError> {
    let terminal = manager.resolve_terminal(process_id).await?;
    let context = make_context(session, turn, call_id);
    manager
        .write_stdin(
            &context,
            WriteStdinRequest {
                terminal: &terminal,
                input,
                yield_time_ms: 250,
                max_output_tokens,
                truncation_policy: turn.model_info().truncation_policy.into(),
                interaction_event: None,
            },
        )
        .await
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exited_large_output_pages_exact_bytes_and_rejects_non_empty_stdin() -> anyhow::Result<()> {
    skip_if_sandbox!(Ok(()));

    let codex_home = TempDir::new()?;
    let thread_id = ThreadId::new();
    let manager = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        thread_id,
        /*ephemeral*/ false,
        /*forked_from_thread_id*/ None,
    );
    let (session, turn) = test_session_and_turn().await;

    let expected: String = (0..40)
        .map(|i| format!("row-{i:03}: 🦀 payload-line\n"))
        .collect();
    let cmd = format!(
        "python3 -c {}",
        shlex::try_quote(&format!("import sys; sys.stdout.write({expected:?})"))?
    );

    let first = run_exec(&manager, &session, &turn, "exec-1", &cmd, 5_000, Some(30)).await?;
    let session_id = first
        .process_id
        .expect("oversized exited command should return session_id for paging");
    assert_eq!(first.exit_code, Some(0));
    assert!(first.emit_post_tool_use);

    // Non-empty write_stdin to an exited/retained session is rejected without advancing cursor.
    let err = run_write_stdin(
        &manager,
        &session,
        &turn,
        "write-bad",
        session_id,
        "hi\n",
        Some(30),
    )
    .await
    .expect_err("non-empty stdin to exited retained session must fail");
    assert!(
        matches!(err, UnifiedExecError::ProcessExited { process_id } if process_id == session_id)
    );

    let mut assembled = first.raw_output.text.clone();
    let mut next_id = Some(session_id);
    let mut pages = 1usize;
    while let Some(id) = next_id {
        let page = run_write_stdin(
            &manager,
            &session,
            &turn,
            &format!("page-{pages}"),
            id,
            "",
            Some(30),
        )
        .await?;
        assert_eq!(page.exit_code, Some(0));
        assert!(!page.emit_post_tool_use);
        assert!(!page.raw_output.text.contains("tokens truncated"));
        assert!(!page.raw_output.text.contains("bytes omitted"));
        assembled.push_str(&page.raw_output.text);
        next_id = page.process_id;
        pages += 1;
        assert!(pages < 100);
    }

    assert!(pages > 2, "expected multi-page output, got {pages}");
    assert_eq!(assembled, expected);

    // After final page omits session_id, subsequent lookup returns UnknownProcessId.
    let after = manager.resolve_terminal(session_id).await;
    assert!(
        matches!(after, Err(UnifiedExecError::UnknownProcessId { process_id }) if process_id == session_id)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = retained_exec_thread_dir(codex_home.path(), thread_id);
        let dir_mode = fs::metadata(&dir)?.permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let bin_mode = fs::metadata(dir.join(format!("{session_id}.bin")))?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(bin_mode, 0o600);
    }

    Ok(())
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_process_with_buffered_output_pages_immediately_without_empty_yield_stall()
-> anyhow::Result<()> {
    skip_if_sandbox!(Ok(()));

    let codex_home = TempDir::new()?;
    let thread_id = ThreadId::new();
    let manager = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        thread_id,
        /*ephemeral*/ false,
        /*forked_from_thread_id*/ None,
    );
    let (session, turn) = test_session_and_turn().await;

    let marker_dir = TempDir::new()?;
    let release_file = marker_dir.path().join("release");
    let expected: String = (0..30)
        .map(|i| format!("live-line-{i:03} abcdefghij\n"))
        .collect();
    let py = format!(
        "import os, sys, time; sys.stdout.write({expected:?}); sys.stdout.flush()\n\
         while not os.path.exists({:?}): time.sleep(0.02)\n\
         sys.stdout.write('DONE\\n'); sys.stdout.flush()",
        release_file.to_string_lossy()
    );
    let cmd = format!("python3 -c {}", shlex::try_quote(&py)?);

    let first = run_exec(&manager, &session, &turn, "exec-live", &cmd, 300, Some(25)).await?;
    let session_id = first.process_id.expect("running process session_id");
    assert_eq!(first.exit_code, None);
    assert!(!first.emit_post_tool_use);

    // Buffered unread bytes must be returned immediately without waiting MIN_EMPTY_YIELD_TIME_MS (5s).
    let page_start = Instant::now();
    let second = run_write_stdin(
        &manager,
        &session,
        &turn,
        "page-fast",
        session_id,
        "",
        Some(25),
    )
    .await?;
    let elapsed = page_start.elapsed();
    assert!(
        elapsed < Duration::from_millis(MIN_EMPTY_YIELD_TIME_MS / 2),
        "empty write_stdin stalled while unread buffered output was available: {elapsed:?}"
    );
    assert_eq!(second.process_id, Some(session_id));
    assert_eq!(second.exit_code, None);
    assert!(!second.raw_output.text.is_empty());

    let mut assembled = first.raw_output.text + &second.raw_output.text;

    // Allow the process to exit and finish reading all remaining pages.
    fs::write(&release_file, b"go")?;
    let mut next_id = Some(session_id);
    let mut saw_exit_code = None;
    let mut exit_post_tool_use_count = 0usize;
    let mut guard_iters = 0usize;
    while let Some(id) = next_id {
        let page = run_write_stdin(
            &manager,
            &session,
            &turn,
            &format!("drain-{guard_iters}"),
            id,
            "",
            Some(25),
        )
        .await?;
        assembled.push_str(&page.raw_output.text);
        if page.exit_code.is_some() {
            saw_exit_code = page.exit_code;
        }
        if page.emit_post_tool_use {
            exit_post_tool_use_count += 1;
        }
        next_id = page.process_id;
        guard_iters += 1;
        assert!(guard_iters < 100);
    }

    assert_eq!(saw_exit_code, Some(0));
    assert_eq!(exit_post_tool_use_count, 1);
    assert_eq!(assembled, format!("{expected}DONE\n"));

    Ok(())
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_thread_scope_and_fork_read_only_isolation() -> anyhow::Result<()> {
    skip_if_sandbox!(Ok(()));

    let codex_home = TempDir::new()?;
    let parent_thread_id = ThreadId::new();
    let parent_manager = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        parent_thread_id,
        /*ephemeral*/ false,
        /*forked_from_thread_id*/ None,
    );
    let (session, turn) = test_session_and_turn().await;

    let expected: String = (0..30)
        .map(|i| format!("persist-{i:03} 🦀 data\n"))
        .collect();
    let cmd = format!(
        "python3 -c {}",
        shlex::try_quote(&format!("import sys; sys.stdout.write({expected:?})"))?
    );

    let first = run_exec(
        &parent_manager,
        &session,
        &turn,
        "exec-p",
        &cmd,
        5_000,
        Some(25),
    )
    .await?;
    let session_id = first.process_id.expect("session_id for paging");
    assert_eq!(first.exit_code, Some(0));

    let parent_dir = retained_exec_thread_dir(codex_home.path(), parent_thread_id);
    let parent_meta_before_fork = fs::read(parent_dir.join(format!("{session_id}.json")))?;
    let parent_bin_before_fork = fs::read(parent_dir.join(format!("{session_id}.bin")))?;

    // 1. Child subagent (different thread_id, NOT forked) cannot resolve parent's session_id.
    let subagent_manager = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        ThreadId::new(),
        /*ephemeral*/ false,
        /*forked_from_thread_id*/ None,
    );
    assert!(matches!(
        subagent_manager.resolve_terminal(session_id).await,
        Err(UnifiedExecError::UnknownProcessId { process_id }) if process_id == session_id
    ));

    // 2. Fork inherits parent's retained session read-only at fork-time cursor.
    let fork_thread_id = ThreadId::new();
    let fork_manager = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        fork_thread_id,
        /*ephemeral*/ false,
        Some(parent_thread_id),
    );
    // Fork rejects non-empty stdin on inherited session.
    let fork_write_err = run_write_stdin(
        &fork_manager,
        &session,
        &turn,
        "fork-bad",
        session_id,
        "nope\n",
        Some(25),
    )
    .await
    .expect_err("fork must reject non-empty stdin on inherited session");
    assert!(
        matches!(fork_write_err, UnifiedExecError::ProcessExited { process_id } if process_id == session_id)
    );

    // Fork reads one page and then a second fork (fork-of-a-fork) forks from it.
    let fork_page_2 = run_write_stdin(
        &fork_manager,
        &session,
        &turn,
        "fork-p2",
        session_id,
        "",
        Some(25),
    )
    .await?;
    assert_eq!(fork_page_2.exit_code, Some(0));
    assert_eq!(fork_page_2.process_id, Some(session_id));

    // Fork-of-a-fork inherits fork's cursor and reads from parent's .bin without copying it.
    let grandchild_fork_manager = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        ThreadId::new(),
        /*ephemeral*/ true,
        Some(fork_thread_id),
    );
    let mut grandchild_assembled = first.raw_output.text.clone() + &fork_page_2.raw_output.text;
    let mut next_gc = Some(session_id);
    while let Some(id) = next_gc {
        let p = run_write_stdin(
            &grandchild_fork_manager,
            &session,
            &turn,
            "gc-page",
            id,
            "",
            Some(25),
        )
        .await?;
        grandchild_assembled.push_str(&p.raw_output.text);
        next_gc = p.process_id;
    }
    assert_eq!(grandchild_assembled, expected);

    // Finish reading on first fork too.
    let mut fork_assembled = first.raw_output.text.clone() + &fork_page_2.raw_output.text;
    let mut next_fork = fork_page_2.process_id;
    while let Some(id) = next_fork {
        let p = run_write_stdin(
            &fork_manager,
            &session,
            &turn,
            "fork-rest",
            id,
            "",
            Some(25),
        )
        .await?;
        fork_assembled.push_str(&p.raw_output.text);
        next_fork = p.process_id;
    }
    assert_eq!(fork_assembled, expected);

    // Parent's files on disk must be completely untouched by fork reads!
    assert_eq!(
        fs::read(parent_dir.join(format!("{session_id}.json")))?,
        parent_meta_before_fork
    );
    assert_eq!(
        fs::read(parent_dir.join(format!("{session_id}.bin")))?,
        parent_bin_before_fork
    );

    // 3. Stop and resume parent (fresh manager, no live processes): continues paging same session_id!
    parent_manager.terminate_all_processes().await;
    drop(parent_manager);
    let resumed_parent = UnifiedExecProcessManager::for_thread(
        30_000,
        codex_home.path().to_path_buf(),
        parent_thread_id,
        /*ephemeral*/ false,
        /*forked_from_thread_id*/ None,
    );

    // Resumed manager must not reallocate retained session_id.
    for _ in 0..32 {
        let allocated = resumed_parent.allocate_process_id().await?;
        assert_ne!(allocated, session_id);
        resumed_parent.release_process_id(allocated).await;
    }

    // Resumed manager rejects non-empty stdin and delivers exact remaining bytes.
    let resume_write_err = run_write_stdin(
        &resumed_parent,
        &session,
        &turn,
        "resume-bad",
        session_id,
        "data\n",
        Some(25),
    )
    .await
    .expect_err("resumed session must reject non-empty stdin");
    assert!(
        matches!(resume_write_err, UnifiedExecError::ProcessExited { process_id } if process_id == session_id)
    );

    let mut resumed_assembled = first.raw_output.text.clone();
    let mut next_resumed = Some(session_id);
    while let Some(id) = next_resumed {
        let p = run_write_stdin(
            &resumed_parent,
            &session,
            &turn,
            "resumed-page",
            id,
            "",
            Some(25),
        )
        .await?;
        assert_eq!(p.exit_code, Some(0));
        assert!(!p.emit_post_tool_use);
        resumed_assembled.push_str(&p.raw_output.text);
        next_resumed = p.process_id;
    }
    assert_eq!(resumed_assembled, expected);

    Ok(())
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oneshot_exec_command_to_completion_retains_and_pages_oversized_output()
-> anyhow::Result<()> {
    skip_if_sandbox!(Ok(()));

    let (session, turn) = test_session_and_turn().await;
    let manager = &session.services.unified_exec_manager;
    let process_id = manager.allocate_process_id().await?;

    let expected: String = (0..25)
        .map(|i| format!("oneshot-{i:03} payload\n"))
        .collect();
    let cmd = format!(
        "python3 -c {}",
        shlex::try_quote(&format!("import sys; sys.stdout.write({expected:?})"))?
    );
    let prepared =
        prepared_bash_command(&turn, &cmd, 1_000, Some(20), Some(Duration::from_secs(10)));
    let context = make_context(&session, &turn, "oneshot-1");
    let first = UnifiedExecProcessManager::exec_command_to_completion(
        ExecCommandRequest {
            prepared,
            process_id,
            network: None,
        },
        &context,
        Duration::from_secs(10),
    )
    .await?;

    let session_id = first
        .process_id
        .expect("oversized one-shot completion should return session_id for paging");
    assert_eq!(first.exit_code, Some(0));

    let mut assembled = first.raw_output.text;
    let mut next_id = Some(session_id);
    while let Some(id) = next_id {
        let page =
            run_write_stdin(manager, &session, &turn, "oneshot-page", id, "", Some(20)).await?;
        assert_eq!(page.exit_code, Some(0));
        assembled.push_str(&page.raw_output.text);
        next_id = page.process_id;
    }
    assert_eq!(assembled, expected);

    Ok(())
}

#[test]
fn retained_storage_rejects_missing_dir_gracefully() {
    let dir = TempDir::new().expect("temp dir");
    let storage = super::RetainedExecStorage::new(
        dir.path().to_path_buf(),
        ThreadId::new(),
        /*ephemeral*/ false,
    );
    let (reserved, entries) = storage.load_own_entries();
    assert!(reserved.is_empty());
    assert!(entries.is_empty());
    assert_eq!(TruncationPolicy::Tokens(10).byte_budget(), 40);
}
