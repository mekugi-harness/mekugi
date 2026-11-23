use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ShellGuardApprovalDecision;
use codex_app_server_protocol::ShellGuardApprovalKind;
use codex_app_server_protocol::ShellGuardRequestApprovalResponse;
use codex_app_server_protocol::ShellGuardRequestSecretResponse;
use codex_app_server_protocol::ShellGuardSecretDecision;
use codex_app_server_protocol::ShellGuardSecretKind;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_features::Feature;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

#[cfg(target_os = "macos")]
const READ_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(not(target_os = "macos"))]
const READ_TIMEOUT: Duration = Duration::from_secs(20);

fn write_executable(dir: &Path, name: &str, body: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, body)?;
    let mut perms = std::fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms)?;
    Ok(path)
}

fn command_environment(mock_bin: &Path) -> String {
    format!(
        r#"[shell_environment_policy.set]
PATH = "{}:/usr/bin:/bin"
GIT_CONFIG_NOSYSTEM = "1"
GIT_CONFIG_GLOBAL = "/dev/null"
"#,
        mock_bin.display()
    )
}

fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

async fn start_turn(
    app: &mut TestAppServer,
    thread_id: &str,
    text: &str,
) -> Result<TurnStartResponse> {
    timeout(
        READ_TIMEOUT,
        app.request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread_id.to_string(),
                input: vec![V2UserInput::Text {
                    text: text.to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        }),
    )
    .await?
}

fn collect_rollout_text(codex_home: &Path) -> Result<String> {
    let mut out = String::new();
    let sessions = codex_home.join("sessions");
    if !sessions.exists() {
        return Ok(out);
    }
    let mut stack = vec![sessions];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push_str(&std::fs::read_to_string(&path)?);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// Under `--yolo` (`approval_policy = "never"`, `sandbox_mode = "danger-full-access"`),
/// remote writes prompt via `shellGuard/requestApproval` for bare, absolute, `env -i`,
/// and nested `-c` invocations; unreached branches and non-writes never prompt; denial
/// exits 1 without running the tool while allowing subsequent `;` commands to run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_guard_remote_write_accept_deny_and_non_write_under_yolo() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mock_bin = TempDir::new()?;
    let mock_git = write_executable(
        mock_bin.path(),
        "git",
        r#"#!/bin/bash
for arg in "$@"; do
    if [[ $arg = config ]]; then exec /usr/bin/git "$@"; fi
done
printf 'MOCK_GIT:%s\n' "$*"
"#,
    )?;
    for name in ["gh", "hg", "svn", "jj"] {
        write_executable(
            mock_bin.path(),
            name,
            "#!/bin/bash\nprintf 'RAN:%s\\n' \"$*\"\n",
        )?;
    }

    let model_server = responses::start_mock_server().await;
    let abs_git = shell_single_quote(&mock_git.to_string_lossy());
    let script = format!(
        "false && git push origin unreached; \
         git -c alias.local=status local; \
         git status; \
         git push --dry-run origin main; \
         git push origin allowed; \
         git push origin blocked; \
         echo AFTER_DENY:$?; \
         git -c alias.ship=push ship origin alias; \
         timeout 30 git push origin timed; \
         command -p git push origin standard || echo STANDARD_DENIED:$?; \
         gh pr create; hg push; svn commit; jj git push; \
         {abs_git} push origin abs; \
         env -i PATH=/usr/bin:/bin {abs_git} push origin cleanenv; \
         sh -c '{abs_git_inner} push origin nested'",
        abs_git = abs_git,
        abs_git_inner = mock_git.to_string_lossy(),
    );
    let exec_code = format!(
        r#"// @exec: {{"yield_time_ms": 60000}}
const res = await tools.exec_command({{cmd: {cmd}, shell: "/bin/bash", login: false}});
text(res.output);"#,
        cmd = serde_json::to_string(&script)?,
    );
    let events = vec![
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_custom_tool_call("call-1", "exec", &exec_code),
            responses::ev_completed("resp-1"),
        ]),
        create_final_assistant_message_sse_response("done")?,
    ];
    let mock = responses::mount_sse_sequence(&model_server, events).await;

    MockResponsesConfig::new(&model_server.uri())
        .with_approval_policy("never")
        .with_sandbox_mode("danger-full-access")
        .enable_feature(Feature::CodeModeOnly)
        .with_extra_config(&command_environment(mock_bin.path()))
        .write(codex_home.path())?;

    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[
            ("SHELL", Some("/bin/bash")),
            ("PATH", Some("/usr/bin:/bin")),
            ("BASH_ENV", None),
        ])
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app.start_thread(ThreadStartParams::default()).await?;
    let _turn = start_turn(&mut app, &thread.id, "run git commands").await?;

    for (tool, expected) in [
        ("git", "allowed"),
        ("git", "blocked"),
        ("git", "alias"),
        ("git", "timed"),
        ("git", "standard"),
        ("gh", "create"),
        ("hg", "push"),
        ("svn", "commit"),
        ("jj", "push"),
        ("git", "abs"),
        ("git", "cleanenv"),
        ("git", "nested"),
    ] {
        let req = timeout(READ_TIMEOUT, app.read_stream_until_request_message()).await??;
        let ServerRequest::ShellGuardRequestApproval { request_id, params } = req else {
            anyhow::bail!("unexpected server request: {req:?}");
        };
        assert_eq!(params.thread_id, thread.id);
        assert_eq!(params.kind, ShellGuardApprovalKind::RemoteWrite);
        if expected == "standard" {
            assert_eq!(
                std::fs::canonicalize(params.executable)?,
                std::fs::canonicalize("/usr/bin/git")?
            );
        } else {
            assert_eq!(
                params.executable,
                mock_bin.path().join(tool).to_string_lossy()
            );
        }
        let target = params.argv.last().cloned().unwrap_or_default();
        assert_eq!(target, expected, "unexpected argv: {:?}", params.argv);

        let response = if matches!(expected, "blocked" | "standard") {
            ShellGuardRequestApprovalResponse {
                decision: ShellGuardApprovalDecision::Decline,
                reason: Some("keep local".to_string()),
            }
        } else {
            ShellGuardRequestApprovalResponse {
                decision: ShellGuardApprovalDecision::Accept,
                reason: None,
            }
        };
        app.send_response(request_id, serde_json::to_value(response)?)
            .await?;
    }

    timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = mock.requests();
    let follow_up_body = requests
        .get(1)
        .context("missing follow-up model request")?
        .body_json();
    let follow_up_text = serde_json::to_string(&follow_up_body)?;
    assert!(
        follow_up_text.contains("MOCK_GIT:status"),
        "expected non-write git status: {follow_up_text}"
    );
    assert!(
        follow_up_text.contains("MOCK_GIT:push --dry-run origin main"),
        "expected dry-run push: {follow_up_text}"
    );
    assert!(
        follow_up_text.contains("MOCK_GIT:push origin allowed"),
        "expected allowed push: {follow_up_text}"
    );
    assert!(
        !follow_up_text.contains("MOCK_GIT:push origin blocked"),
        "denied push must not execute: {follow_up_text}"
    );
    assert!(
        !follow_up_text.contains("MOCK_GIT:push origin unreached"),
        "unreached branch must not execute: {follow_up_text}"
    );
    assert!(
        follow_up_text.contains("keep local"),
        "denial reason should reach stderr: {follow_up_text}"
    );
    assert!(
        follow_up_text.contains("AFTER_DENY:1") && follow_up_text.contains("STANDARD_DENIED:1"),
        "commands after denied write must continue: {follow_up_text}"
    );
    assert!(follow_up_text.contains("MOCK_GIT:-c alias.local=status local"));
    assert!(follow_up_text.contains("MOCK_GIT:-c alias.ship=push ship origin alias"));
    for expected in ["RAN:pr create", "RAN:push", "RAN:commit", "RAN:git push"] {
        assert!(follow_up_text.contains(expected));
    }
    for label in ["timed", "abs", "cleanenv", "nested"] {
        assert!(
            follow_up_text.contains(&format!("MOCK_GIT:push origin {label}")),
            "expected {label} push to run: {follow_up_text}"
        );
    }
    assert!(
        !follow_up_text.contains("--codex-shell-guard"),
        "internal wrapper flags leaked to model output: {follow_up_text}"
    );

    app.shutdown_gracefully().await?;
    Ok(())
}

/// `sudo` prompts for command approval then secret askpass; `gpg` prompts for passphrase
/// via `--command-fd`/`--status-fd`; secrets never appear in model input or rollouts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_guard_sudo_and_gpg_secrets_never_leak() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mock_bin = TempDir::new()?;
    let sudo_secret = "top-secret-sudo-pw-9981";
    let gpg_secret = "top-secret-gpg-phrase-7742";

    let mock_sudo = write_executable(
        mock_bin.path(),
        "sudo",
        &format!(
            r#"#!/bin/bash
case "$1" in
    -n|--non-interactive) echo "NO_PASSWORD:$1" >&2; exit 42 ;;
esac
if [ "$1" != "-A" ]; then
  echo "MISSING_ASKPASS_FLAG:$*" >&2
  exit 42
fi
pw=$("$SUDO_ASKPASS" 'Password:') || exit 43
if [ "$pw" != "{sudo_secret}" ]; then
  echo "WRONG_SUDO_PW" >&2
  exit 44
fi
echo "SUDO_UNLOCKED:$2"
"#
        ),
    )?;

    let mock_gpg = write_executable(
        mock_bin.path(),
        "gpg",
        &format!(
            r#"#!/bin/bash
if [ "$1" = "--no-options" ] && [ "$2" = "--dump-option-table" ]; then
  printf 'sign:115:0:sign\ncommand-fd:0:1:fd\nstatus-fd:0:1:fd\npinentry-mode:0:2:mode\n'
  exit 0
fi
status_fd=""
pass_fd=""
prev=""
for arg in "$@"; do
  case "$prev" in
    --status-fd) status_fd="$arg"; prev=""; continue ;;
    --command-fd) pass_fd="$arg"; prev=""; continue ;;
  esac
  case "$arg" in
    --status-fd|--command-fd) prev="$arg" ;;
    --status-fd=*) status_fd="${{arg#--status-fd=}}" ;;
    --command-fd=*) pass_fd="${{arg#--command-fd=}}" ;;
  esac
done
if [ -z "$status_fd" ] || [ -z "$pass_fd" ]; then
  echo "MISSING_GPG_FDS:$*" >&2
  exit 45
fi
printf '[GNUPG:] GET_HIDDEN passphrase.enter\n' >&"$status_fd"
IFS= read -r got <&"$pass_fd" || exit 46
if [ "$got" != "{gpg_secret}" ]; then
  echo "WRONG_GPG_PW" >&2
  exit 47
fi
printf '[GNUPG:] GOOD_PASSPHRASE\n' >&"$status_fd"
echo "GPG_SIGNED_OK"
"#
        ),
    )?;

    let model_server = responses::start_mock_server().await;
    let exec_code = r#"// @exec: {"yield_time_ms": 60000}
const r1 = await tools.exec_command({shell: "/bin/bash", login: false, cmd: "sudo -n id; echo SHORT_STATUS:$?; sudo --non-interactive id; echo LONG_STATUS:$?; sudo id"});
const r2 = await tools.exec_command({shell: "/bin/bash", login: false, cmd: "gpg --sign doc.txt"});
const denied = await tools.exec_command({shell: "/bin/bash", login: false, cmd: "gpg --sign doc.txt"});
text(r1.output + "\n" + r2.output + "\nDENIED_GPG:" + denied.exit_code);"#;
    let events = vec![
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_custom_tool_call("call-1", "exec", exec_code),
            responses::ev_completed("resp-1"),
        ]),
        create_final_assistant_message_sse_response("done")?,
    ];
    let mock = responses::mount_sse_sequence(&model_server, events).await;

    MockResponsesConfig::new(&model_server.uri())
        .with_approval_policy("never")
        .with_sandbox_mode("danger-full-access")
        .enable_feature(Feature::CodeModeOnly)
        .with_extra_config(&command_environment(mock_bin.path()))
        .write(codex_home.path())?;

    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[
            ("SHELL", Some("/bin/bash")),
            ("PATH", Some("/usr/bin:/bin")),
            ("BASH_ENV", None),
        ])
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app.start_thread(ThreadStartParams::default()).await?;
    let _turn = start_turn(&mut app, &thread.id, "run sudo and gpg").await?;

    // 1) sudo command approval
    let req = timeout(READ_TIMEOUT, app.read_stream_until_request_message()).await??;
    let ServerRequest::ShellGuardRequestApproval { request_id, params } = req else {
        anyhow::bail!("expected sudo approval, got {req:?}");
    };
    assert_eq!(params.kind, ShellGuardApprovalKind::Sudo);
    assert_eq!(params.executable, mock_sudo.to_string_lossy());
    assert_eq!(params.argv, vec!["sudo", "id"]);
    app.send_response(
        request_id,
        serde_json::to_value(ShellGuardRequestApprovalResponse {
            decision: ShellGuardApprovalDecision::Accept,
            reason: None,
        })?,
    )
    .await?;

    // 2) sudo password secret prompt
    let req = timeout(READ_TIMEOUT, app.read_stream_until_request_message()).await??;
    let ServerRequest::ShellGuardRequestSecret { request_id, params } = req else {
        anyhow::bail!("expected sudo secret prompt, got {req:?}");
    };
    assert_eq!(params.kind, ShellGuardSecretKind::SudoPassword);
    app.send_response(
        request_id,
        serde_json::to_value(ShellGuardRequestSecretResponse {
            decision: ShellGuardSecretDecision::Provide,
            secret: Some(sudo_secret.to_string()),
            reason: None,
        })?,
    )
    .await?;

    // 3) gpg passphrase secret prompt
    let req = timeout(READ_TIMEOUT, app.read_stream_until_request_message()).await??;
    let ServerRequest::ShellGuardRequestSecret { request_id, params } = req else {
        anyhow::bail!("expected gpg secret prompt, got {req:?}");
    };
    assert_eq!(params.kind, ShellGuardSecretKind::GpgPassphrase);
    assert_eq!(params.executable, mock_gpg.to_string_lossy());
    app.send_response(
        request_id,
        serde_json::to_value(ShellGuardRequestSecretResponse {
            decision: ShellGuardSecretDecision::Provide,
            secret: Some(gpg_secret.to_string()),
            reason: None,
        })?,
    )
    .await?;

    let req = timeout(READ_TIMEOUT, app.read_stream_until_request_message()).await??;
    let ServerRequest::ShellGuardRequestSecret { request_id, params } = req else {
        anyhow::bail!("expected second gpg passphrase request");
    };
    assert_eq!(params.kind, ShellGuardSecretKind::GpgPassphrase);
    app.send_response(request_id, serde_json::json!({"decision":"decline"}))
        .await?;

    timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = mock.requests();
    assert!(
        serde_json::to_string(&requests[0].body_json())?
            .contains("without `-n` or `--non-interactive`")
    );
    let follow_up_text = serde_json::to_string(
        &requests
            .get(1)
            .context("missing follow-up model request")?
            .body_json(),
    )?;
    assert!(
        follow_up_text.contains("SUDO_UNLOCKED:id"),
        "sudo did not complete: {follow_up_text}"
    );
    for native_result in [
        "DENIED_GPG:1",
        "NO_PASSWORD:-n",
        "SHORT_STATUS:42",
        "NO_PASSWORD:--non-interactive",
        "LONG_STATUS:42",
    ] {
        assert!(
            follow_up_text.contains(native_result),
            "missing {native_result}"
        );
    }
    assert!(
        follow_up_text.contains("GPG_SIGNED_OK"),
        "gpg did not complete: {follow_up_text}"
    );
    assert!(
        !follow_up_text.contains(sudo_secret) && !follow_up_text.contains(gpg_secret),
        "secret leaked into model input"
    );

    app.shutdown_gracefully().await?;

    let rollout = collect_rollout_text(codex_home.path())?;
    assert!(
        !rollout.contains(sudo_secret) && !rollout.contains(gpg_secret),
        "secret leaked into rollout"
    );
    assert!(
        !rollout.contains("shell_guard_secret"),
        "shell guard secret event persisted to rollout"
    );
    Ok(())
}

/// `-c features.shell_guard=false` disables shell guard shims and prompts cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_guard_disabled_via_feature_flag() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mock_bin = TempDir::new()?;
    write_executable(
        mock_bin.path(),
        "git",
        "#!/bin/bash\nprintf 'UNGUARDED_GIT:%s\\n' \"$*\"\n",
    )?;
    write_executable(
        mock_bin.path(),
        "sudo",
        "#!/bin/bash\nprintf 'UNGUARDED_SUDO:%s\\n' \"$*\"\n",
    )?;

    let model_server = responses::start_mock_server().await;
    let exec_code = r#"// @exec: {"yield_time_ms": 60000}
const res = await tools.exec_command({shell: "/bin/bash", login: false, cmd: "git push origin main && sudo id"});
text(res.output);"#;
    let events = vec![
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_custom_tool_call("call-1", "exec", exec_code),
            responses::ev_completed("resp-1"),
        ]),
        create_final_assistant_message_sse_response("done")?,
    ];
    let mock = responses::mount_sse_sequence(&model_server, events).await;

    MockResponsesConfig::new(&model_server.uri())
        .with_approval_policy("never")
        .with_sandbox_mode("danger-full-access")
        .enable_feature(Feature::CodeModeOnly)
        .with_extra_config(&command_environment(mock_bin.path()))
        .write(codex_home.path())?;

    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_args(&["-c", "features.shell_guard=false"])
        .with_env_overrides(&[
            ("SHELL", Some("/bin/bash")),
            ("PATH", Some("/usr/bin:/bin")),
            ("BASH_ENV", None),
        ])
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app.start_thread(ThreadStartParams::default()).await?;
    let _turn = start_turn(&mut app, &thread.id, "run unguarded commands").await?;

    timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let follow_up_text = serde_json::to_string(
        &mock
            .requests()
            .get(1)
            .context("missing follow-up model request")?
            .body_json(),
    )?;
    assert!(
        !serde_json::to_string(&mock.requests()[0].body_json())?
            .contains("without `-n` or `--non-interactive`")
    );
    assert!(follow_up_text.contains("UNGUARDED_GIT:push origin main"));
    assert!(follow_up_text.contains("UNGUARDED_SUDO:id"));

    app.shutdown_gracefully().await?;
    Ok(())
}

/// User `PreToolUse` hooks remain composable with native shell guard: when a user hook
/// rewrites a command into a remote write, native shell guard still prompts after the hook.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_guard_composes_with_user_pre_tool_use_hook() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mock_bin = TempDir::new()?;
    let mock_git = write_executable(
        mock_bin.path(),
        "git",
        "#!/bin/bash\nprintf 'HOOKED_GIT:%s\\n' \"$*\"\n",
    )?;
    let hook_script = write_executable(
        mock_bin.path(),
        "rewrite-hook.sh",
        r#"#!/bin/bash
cat >/dev/null
printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"git push origin from-hook"}}}\n'
"#,
    )?;

    let model_server = responses::start_mock_server().await;
    let exec_code = r#"// @exec: {"yield_time_ms": 60000}
const res = await tools.exec_command({shell: "/bin/bash", login: false, cmd: "echo safe"});
text(res.output);"#;
    let events = vec![
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_custom_tool_call("call-1", "exec", exec_code),
            responses::ev_completed("resp-1"),
        ]),
        create_final_assistant_message_sse_response("done")?,
    ];
    let mock = responses::mount_sse_sequence(&model_server, events).await;

    MockResponsesConfig::new(&model_server.uri())
        .with_approval_policy("never")
        .with_sandbox_mode(if cfg!(target_os = "macos") { "read-only" } else { "danger-full-access" })
        .enable_feature(Feature::CodeModeOnly)
        .with_extra_config(&command_environment(mock_bin.path()))
        .with_extra_config(&format!(
            "[hooks]\nPreToolUse = [{{ matcher = \"^Bash$\", hooks = [{{ type = \"command\", command = {:?}, timeout = 5, async = false }}] }}]",
            hook_script.to_string_lossy()
        ))
        .write(codex_home.path())?;

    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[
            ("SHELL", Some("/bin/bash")),
            ("PATH", Some("/usr/bin:/bin")),
            ("BASH_ENV", None),
        ])
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app
        .start_thread(ThreadStartParams {
            config: Some(std::collections::HashMap::from([(
                "bypass_hook_trust".to_owned(),
                serde_json::json!(true),
            )])),
            ..Default::default()
        })
        .await?;
    let _turn = start_turn(&mut app, &thread.id, "run hooked command").await?;

    let req = timeout(READ_TIMEOUT, app.read_stream_until_request_message()).await??;
    let ServerRequest::ShellGuardRequestApproval { request_id, params } = req else {
        anyhow::bail!("expected shell guard approval after user hook, got {req:?}");
    };
    assert_eq!(params.kind, ShellGuardApprovalKind::RemoteWrite);
    assert_eq!(params.executable, mock_git.to_string_lossy());
    assert_eq!(params.argv, vec!["git", "push", "origin", "from-hook"]);
    app.send_response(
        request_id,
        serde_json::to_value(ShellGuardRequestApprovalResponse {
            decision: ShellGuardApprovalDecision::Accept,
            reason: None,
        })?,
    )
    .await?;

    timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let follow_up_text = serde_json::to_string(
        &mock
            .requests()
            .get(1)
            .context("missing follow-up model request")?
            .body_json(),
    )?;
    assert!(
        follow_up_text.contains("HOOKED_GIT:push origin from-hook"),
        "expected hooked remote write to execute after approval: {follow_up_text}"
    );

    app.shutdown_gracefully().await?;
    Ok(())
}

/// When the approval socket is unreachable, shims fail closed (exit 1 without executing
/// the guarded tool) while allowing subsequent `;` commands to continue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_guard_fail_closed_when_socket_unreachable() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mock_bin = TempDir::new()?;
    write_executable(
        mock_bin.path(),
        "git",
        "#!/bin/bash\nprintf 'MUST_NOT_RUN\\n'\n",
    )?;

    let model_server = responses::start_mock_server().await;
    let exec_code = r#"// @exec: {"yield_time_ms": 60000}
const res = await tools.exec_command({shell: "/bin/bash", login: false, cmd: "CODEX_SHELL_GUARD_SOCKET=/nonexistent/codex-sg.sock git push origin main; echo CONTINUED:$?"});
text(res.output);"#;
    let events = vec![
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_custom_tool_call("call-1", "exec", exec_code),
            responses::ev_completed("resp-1"),
        ]),
        create_final_assistant_message_sse_response("done")?,
    ];
    let mock = responses::mount_sse_sequence(&model_server, events).await;

    MockResponsesConfig::new(&model_server.uri())
        .with_approval_policy("never")
        .with_sandbox_mode("danger-full-access")
        .enable_feature(Feature::CodeModeOnly)
        .with_extra_config(&command_environment(mock_bin.path()))
        .write(codex_home.path())?;

    let mut app = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .with_env_overrides(&[
            ("SHELL", Some("/bin/bash")),
            ("PATH", Some("/usr/bin:/bin")),
            ("BASH_ENV", None),
        ])
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = app.start_thread(ThreadStartParams::default()).await?;
    let _turn = start_turn(&mut app, &thread.id, "run unreachable guard socket").await?;

    timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let follow_up_text = serde_json::to_string(
        &mock
            .requests()
            .get(1)
            .context("missing follow-up model request")?
            .body_json(),
    )?;
    assert!(
        !follow_up_text.contains("MUST_NOT_RUN"),
        "unreachable guard socket executed guarded tool: {follow_up_text}"
    );
    assert!(
        follow_up_text.contains("CONTINUED:1")
            && follow_up_text.contains("Mekugi approval is unavailable"),
        "command after fail-closed denial did not continue: {follow_up_text}"
    );

    app.shutdown_gracefully().await?;
    Ok(())
}
