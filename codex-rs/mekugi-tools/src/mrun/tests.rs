use pretty_assertions::assert_eq;

use super::execute;

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

#[test]
fn argument_validation_and_exit_codes() {
    // Missing budget -> exit 2
    let out = execute(&s(&["echo", "hi"]));
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.starts_with("mrun: "));

    // Invalid --max-tokens -> exit 2
    for bad in [
        "--max-tokens=",
        "--max-tokens=0",
        "--max-tokens=01",
        "--max-tokens=abc",
    ] {
        let out = execute(&s(&[bad, "echo", "hi"]));
        assert_eq!(out.exit_code, 2);
        assert!(
            out.stderr
                .contains("--max-tokens requires one positive integer")
        );
    }

    // Repeated --head/--tail -> exit 2
    let out = execute(&s(&["-n", "2", "--head", "--tail", "echo", "hi"]));
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.contains("specify --head or --tail at most once"));

    // Missing command -> exit 127
    let out = execute(&s(&["-n", "2", "definitely_nonexistent_mekugi_cmd_xyz"]));
    assert_eq!(out.exit_code, 127);

    // Child options after command name are passed untouched
    let out = execute(&s(&["-n", "2", "printf", "%s\n", "--max-tokens=99"]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "--max-tokens=99\n");
    assert_eq!(out.stderr, "");
}

#[test]
fn line_mode_head_and_tail_discard() {
    let script = "printf 'L1\\nL2\\nL3\\nL4\\nL5\\n'";

    // Default is --tail
    let out = execute(&s(&["-n", "2", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "L4\nL5\n");
    assert!(!out.stdout.contains("L1"));
    assert_eq!(
        out.stderr,
        "mrun: output incomplete: 2-line limit reached\n"
    );

    // --head keeps first 2 lines and discards the rest
    let out = execute(&s(&["-n", "2", "--head", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "L1\nL2\n");
    assert!(!out.stdout.contains("L5"));
    assert_eq!(
        out.stderr,
        "mrun: output incomplete: 2-line limit reached\n"
    );

    // Within limit -> no omission notice
    let out = execute(&s(&["-n", "10", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "L1\nL2\nL3\nL4\nL5\n");
    assert_eq!(out.stderr, "");
}

#[test]
fn token_mode_head_tail_and_stderr_half_cap() {
    // 10 tokens = 40 bytes total; when stdout is non-empty, stderr <= 5 tokens (20 bytes)
    let script = "printf '%s' 'OUT_0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ'; printf '%s' 'ERR_0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ' >&2";

    let out = execute(&s(&["--max-tokens", "10", "--head", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.starts_with("OUT_"));
    assert!(out.stdout.len() <= 20, "stdout len={}", out.stdout.len());
    assert!(out.stderr.starts_with("ERR_"));
    let stderr_content = out.stderr.lines().next().unwrap();
    assert!(
        stderr_content.len() <= 20,
        "stderr cap exceeded: {stderr_content:?}"
    );
    assert!(
        out.stderr
            .ends_with("mrun: output incomplete: 10-token limit reached\n")
    );

    // --tail keeps end of streams
    let out = execute(&s(&["--max-tokens=10", "--tail", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.ends_with("XYZ"), "got {:?}", out.stdout);
    let stderr_content = out.stderr.lines().next().unwrap();
    assert!(stderr_content.ends_with("XYZ"), "got {stderr_content:?}");
    assert!(
        out.stderr
            .ends_with("mrun: output incomplete: 10-token limit reached\n")
    );

    // Combined -n and --max-tokens notice wording
    let out = execute(&s(&[
        "-n",
        "2",
        "--max-tokens",
        "5",
        "sh",
        "-c",
        "printf 'a\\nb\\nc\\n'",
    ]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "b\nc\n");
    assert_eq!(
        out.stderr,
        "mrun: output incomplete: 2-line limit or 5-token limit reached\n"
    );
}

#[test]
fn preserves_nonzero_exit_and_unix_signal() {
    let out = execute(&s(&["-n", "2", "sh", "-c", "echo fail; exit 42"]));
    assert_eq!(out.exit_code, 42);
    assert_eq!(out.stdout, "fail\n");
    assert_eq!(out.stderr, "");

    #[cfg(unix)]
    {
        let out = execute(&s(&["-n", "2", "sh", "-c", "kill -9 $$"]));
        assert_eq!(out.exit_code, 128 + 9);
    }
}

#[cfg(unix)]
#[test]
fn retires_descendants_holding_inherited_pipes() {
    let start = std::time::Instant::now();
    let out = execute(&s(&["-n", "2", "sh", "-c", "sleep 60 & echo ready"]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "ready\n");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "mrun hung waiting for grandchild pipe: {:?}",
        start.elapsed()
    );
}

#[test]
fn token_mode_preserves_utf8_boundaries() {
    // 4-byte emoji repeated so byte boundaries cut mid-character in both head and tail modes.
    let script = "printf '%s' 'a🎉🎉🎉b'";

    let head = execute(&s(&["--max-tokens", "1", "--head", "sh", "-c", script]));
    assert_eq!(head.exit_code, 0);
    assert_eq!(head.stdout, "a");
    assert!(
        head.stderr
            .ends_with("mrun: output incomplete: 1-token limit reached\n")
    );

    let tail = execute(&s(&["--max-tokens", "1", "--tail", "sh", "-c", script]));
    assert_eq!(tail.exit_code, 0);
    assert_eq!(tail.stdout, "b");
    assert!(
        tail.stderr
            .ends_with("mrun: output incomplete: 1-token limit reached\n")
    );

    let exact_emoji = execute(&s(&[
        "--max-tokens",
        "1",
        "--tail",
        "sh",
        "-c",
        "printf '%s' '🎉🎉🎉'",
    ]));
    assert_eq!(exact_emoji.exit_code, 0);
    assert_eq!(exact_emoji.stdout, "🎉");
}

#[cfg(unix)]
const PTY_HELPER_ENV: &str = "MEKUGI_MRUN_PTY_TEST_HELPER";

/// Child entrypoint spawned inside an isolated PTY session by `reads_pty_stdin_and_retires_descendants`.
#[cfg(unix)]
#[test]
#[ignore = "internal helper invoked under a real PTY by reads_pty_stdin_and_retires_descendants"]
fn pty_stdin_read_helper() {
    if std::env::var_os(PTY_HELPER_ENV).is_none() {
        return;
    }

    // SAFETY: POSIX queries on stdin and process group.
    let pgrp_before = unsafe { libc::getpgrp() };
    // SAFETY: POSIX query on controlling terminal foreground process group.
    let fg_before = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
    assert_eq!(
        fg_before, pgrp_before,
        "helper must own the PTY foreground before invoking mrun"
    );

    let start = std::time::Instant::now();
    let out = execute(&s(&[
        "-n",
        "5",
        "sh",
        "-c",
        "sleep 60 & printf 'READY\\n' > /dev/tty; IFS= read -r value; printf 'value=%s\\n' \"$value\"",
    ]));
    let elapsed = start.elapsed();

    // SAFETY: POSIX query on controlling terminal foreground process group.
    let fg_after = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
    assert_eq!(
        fg_after, pgrp_before,
        "mrun must restore the caller's foreground process group on exit"
    );
    assert_eq!(out.exit_code, 0, "stderr: {:?}", out.stderr);
    assert_eq!(out.stdout, "value=hello-pty\n");
    assert_eq!(out.stderr, "");
    // `mrun` enforces a 2-second pipe drain timeout, so finishing strictly below 2s proves
    // the `sleep 60` descendant holding inherited stdout/stderr pipes was retired by process-group kill.
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "mrun hung waiting for grandchild process group under PTY: {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn reads_pty_stdin_and_retires_descendants() {
    use std::io::Write;
    use std::process::Command;
    use std::time::Duration;

    let current_exe = std::env::current_exe().expect("resolve current test binary");
    let mut cmd = Command::new(current_exe);
    cmd.args([
        "--exact",
        "mrun::tests::pty_stdin_read_helper",
        "--ignored",
        "--nocapture",
    ])
    .env(PTY_HELPER_ENV, "1");

    let (mut master, mut child) = spawn_in_pty(&mut cmd);
    let mut sent_input = false;
    let (status, transcript) = drive_pty_until_exit(
        &mut master,
        &mut child,
        Duration::from_secs(10),
        |out, m| {
            if !sent_input && out.contains("READY") {
                m.write_all(b"hello-pty\n")
                    .expect("write line to PTY master");
                m.flush().expect("flush PTY master");
                sent_input = true;
            }
        },
    );
    assert!(
        sent_input,
        "child never signaled READY on PTY; transcript:\n{transcript}"
    );
    assert!(
        status.success(),
        "PTY helper failed with {status}; transcript:\n{transcript}"
    );
}

#[cfg(unix)]
fn spawn_in_pty(cmd: &mut std::process::Command) -> (std::fs::File, std::process::Child) {
    use std::os::fd::FromRawFd;
    use std::os::fd::RawFd;
    use std::os::unix::process::CommandExt;

    let mut master_fd: RawFd = -1;
    let mut slave_fd: RawFd = -1;
    // SAFETY: `openpty` writes valid descriptors into `master_fd` and `slave_fd` on success.
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());

    // SAFETY: `openpty` returned two unique owned file descriptors.
    let master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    // Mark both PTY descriptors close-on-exec so parallel tests cannot inherit them.
    // `dup2` in `pre_exec` clears `FD_CLOEXEC` on stdin/stdout/stderr for this child.
    for fd in [master_fd, slave_fd] {
        // SAFETY: `fd` is an open descriptor owned by this test.
        let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_eq!(
            rc,
            0,
            "fcntl(FD_CLOEXEC) failed: {}",
            std::io::Error::last_os_error()
        );
    }

    // SAFETY: Post-fork child setup invokes only async-signal-safe POSIX functions to attach the
    // PTY slave as the new session's controlling terminal.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0 as libc::c_int) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            for target_fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
                if libc::dup2(slave_fd, target_fd) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if slave_fd > libc::STDERR_FILENO {
                libc::close(slave_fd);
            }
            libc::close(master_fd);
            Ok(())
        });
    }

    let child = cmd.spawn().expect("spawn command inside PTY");
    drop(slave);
    (master, child)
}

#[cfg(unix)]
fn drive_pty_until_exit<F>(
    master: &mut std::fs::File,
    child: &mut std::process::Child,
    timeout: std::time::Duration,
    mut on_output: F,
) -> (std::process::ExitStatus, String)
where
    F: FnMut(&str, &mut std::fs::File),
{
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::time::Instant;

    let master_fd = master.as_raw_fd();
    let deadline = Instant::now() + timeout;
    let mut captured = Vec::new();
    let mut exit_status = None;

    while Instant::now() < deadline {
        if exit_status.is_none()
            && let Some(status) = child.try_wait().expect("poll PTY child")
        {
            exit_status = Some(status);
        }

        let mut pollfd = libc::pollfd {
            fd: master_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pollfd` points to a valid single-element `pollfd` struct.
        let ready = unsafe { libc::poll(&mut pollfd, 1, 50) };
        if ready > 0 && (pollfd.revents & (libc::POLLIN | libc::POLLHUP)) != 0 {
            let mut buf = [0u8; 1024];
            match master.read(&mut buf) {
                Ok(0) => {
                    if exit_status.is_some() {
                        break;
                    }
                }
                Ok(n) => {
                    captured.extend_from_slice(&buf[..n]);
                    let transcript = String::from_utf8_lossy(&captured);
                    on_output(&transcript, master);
                }
                Err(err) if err.raw_os_error() == Some(libc::EIO) => {
                    if exit_status.is_some() {
                        break;
                    }
                }
                Err(err) => panic!("read PTY master failed: {err}"),
            }
        } else if exit_status.is_some() {
            break;
        }
    }

    let transcript = String::from_utf8_lossy(&captured).into_owned();
    let status = match exit_status {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PTY child timed out; transcript:\n{transcript}");
        }
    };
    (status, transcript)
}

#[cfg(unix)]
#[test]
#[ignore = "internal helper invoked under a real PTY by ctrl_c tests"]
fn pty_sleep_helper() {
    if std::env::var_os(PTY_HELPER_ENV).is_none() {
        return;
    }
    let out = execute(&s(&[
        "-n",
        "5",
        "sh",
        "-c",
        "printf 'READY\\n' > /dev/tty; sleep 60",
    ]));
    std::process::exit(out.exit_code);
}

#[cfg(unix)]
#[test]
fn ctrl_c_interrupts_shell_command_list_and_pipeline_under_pty() {
    use std::io::Write;
    use std::process::Command;
    use std::time::Duration;

    let current_exe = std::env::current_exe().expect("resolve current test binary");

    // 1. Noninteractive shell command list: Ctrl-C must abort before running the next command.
    let mut list_cmd = Command::new("bash");
    list_cmd
        .args([
            "-c",
            "\"$1\" --exact mrun::tests::pty_sleep_helper --ignored --nocapture; printf 'CONTINUED\\n'",
            "bash",
            current_exe.to_str().expect("utf8 test binary path"),
        ])
        .env(PTY_HELPER_ENV, "1");
    let (mut master, mut child) = spawn_in_pty(&mut list_cmd);
    let mut sent_ctrl_c = false;
    let (status, transcript) = drive_pty_until_exit(
        &mut master,
        &mut child,
        Duration::from_secs(10),
        |out, m| {
            if !sent_ctrl_c && out.contains("READY") {
                m.write_all(&[0x03]).expect("send Ctrl-C");
                m.flush().expect("flush Ctrl-C");
                sent_ctrl_c = true;
            }
        },
    );
    assert!(sent_ctrl_c, "never saw READY; transcript:\n{transcript}");
    assert!(
        !status.success(),
        "outer shell should not succeed after Ctrl-C; transcript:\n{transcript}"
    );
    assert!(
        !transcript.contains("CONTINUED"),
        "outer shell ran command after Ctrl-C; transcript:\n{transcript}"
    );

    // 2. Interactive pipeline: Ctrl-C must interrupt downstream pipeline stage too (status 130, not 0).
    let mut pipe_cmd = Command::new("bash");
    pipe_cmd
        .args(["--norc", "--noprofile", "-i"])
        .env(PTY_HELPER_ENV, "1")
        .env("PS1", "PROMPT> ");
    let (mut master, mut child) = spawn_in_pty(&mut pipe_cmd);
    let exe_str = current_exe.to_str().expect("utf8 test binary path");
    let pipeline = format!(
        "\"{exe_str}\" --exact mrun::tests::pty_sleep_helper --ignored --nocapture | tee /dev/null\n"
    );
    let mut stage = 0u8;
    let (status, transcript) = drive_pty_until_exit(
        &mut master,
        &mut child,
        Duration::from_secs(10),
        |out, m| {
            if stage == 0 && out.contains("PROMPT> ") {
                m.write_all(pipeline.as_bytes()).expect("send pipeline");
                m.flush().expect("flush pipeline");
                stage = 1;
            } else if stage == 1 && out.contains("READY") {
                m.write_all(&[0x03]).expect("send Ctrl-C");
                m.flush().expect("flush Ctrl-C");
                stage = 2;
            } else if stage == 2 && out.matches("PROMPT> ").count() >= 2 {
                m.write_all(b"printf 'PIPE_STATUS=%s\\n' \"$?\"; exit\n")
                    .expect("query pipeline status");
                m.flush().expect("flush status query");
                stage = 3;
            }
        },
    );
    assert_eq!(
        stage, 3,
        "interactive pipeline Ctrl-C stopped at stage {stage}; transcript:\n{transcript}"
    );
    assert!(
        status.success(),
        "interactive bash failed with {status}; transcript:\n{transcript}"
    );
    assert!(
        transcript.contains("PIPE_STATUS=130"),
        "interrupted pipeline did not report status 130; transcript:\n{transcript}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "internal helper invoked under a real PTY by ctrl_z_suspends_and_resumes_under_pty"]
fn pty_ctrl_z_helper() {
    if std::env::var_os(PTY_HELPER_ENV).is_none() {
        return;
    }
    let out = execute(&s(&[
        "-n",
        "5",
        "sh",
        "-c",
        "printf 'READY\\n' > /dev/tty; IFS= read -r value; printf 'resumed=%s\\n' \"$value\"",
    ]));
    print!("DONE:{}", out.stdout);
    std::process::exit(out.exit_code);
}

#[cfg(unix)]
#[test]
fn ctrl_z_suspends_and_resumes_under_pty() {
    use std::io::Write;
    use std::process::Command;
    use std::time::Duration;

    let current_exe = std::env::current_exe().expect("resolve current test binary");
    let exe_str = current_exe.to_str().expect("utf8 test binary path");

    let mut cmd = Command::new("bash");
    cmd.args(["--norc", "--noprofile", "-i"])
        .env(PTY_HELPER_ENV, "1")
        .env("PS1", "PROMPT> ");
    let (mut master, mut child) = spawn_in_pty(&mut cmd);

    let run_line =
        format!("\"{exe_str}\" --exact mrun::tests::pty_ctrl_z_helper --ignored --nocapture\n");
    let mut stage = 0u8;
    let (status, transcript) = drive_pty_until_exit(
        &mut master,
        &mut child,
        Duration::from_secs(10),
        |out, m| {
            if stage == 0 && out.contains("PROMPT> ") {
                m.write_all(run_line.as_bytes()).expect("launch helper");
                m.flush().expect("flush helper");
                stage = 1;
            } else if stage == 1 && out.contains("READY") {
                m.write_all(&[0x1a]).expect("send Ctrl-Z");
                m.flush().expect("flush Ctrl-Z");
                stage = 2;
            } else if stage == 2 && out.contains("Stopped") {
                m.write_all(b"fg\n").expect("send fg");
                m.flush().expect("flush fg");
                stage = 3;
            } else if stage == 3 && out.contains("fg\r\n") {
                m.write_all(b"after-fg\n").expect("send line after fg");
                m.flush().expect("flush line after fg");
                stage = 4;
            } else if stage == 4 && out.contains("DONE:resumed=after-fg") {
                m.write_all(b"exit\n").expect("send exit");
                m.flush().expect("flush exit");
                stage = 5;
            }
        },
    );
    assert_eq!(
        stage, 5,
        "Ctrl-Z/fg flow stopped at stage {stage}; transcript:\n{transcript}"
    );
    assert!(
        status.success(),
        "interactive bash failed with {status}; transcript:\n{transcript}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "internal helper invoked under a real PTY by bg_exit_preserves_shell_foreground_under_pty"]
fn pty_bg_exit_helper() {
    if std::env::var_os(PTY_HELPER_ENV).is_none() {
        return;
    }
    let out = execute(&s(&[
        "-n",
        "5",
        "sh",
        "-c",
        "printf 'READY\\n' > /dev/tty; sleep 0.2; printf 'bg-done\\n'",
    ]));
    print!("DONE:{}", out.stdout);
    std::process::exit(out.exit_code);
}

#[cfg(unix)]
#[test]
fn bg_exit_preserves_shell_foreground_under_pty() {
    use std::io::Write;
    use std::process::Command;
    use std::time::Duration;

    let current_exe = std::env::current_exe().expect("resolve current test binary");
    let exe_str = current_exe.to_str().expect("utf8 test binary path");

    let mut cmd = Command::new("bash");
    cmd.args(["--norc", "--noprofile", "-i"])
        .env(PTY_HELPER_ENV, "1")
        .env("PS1", "PROMPT> ");
    let (mut master, mut child) = spawn_in_pty(&mut cmd);

    let run_line =
        format!("\"{exe_str}\" --exact mrun::tests::pty_bg_exit_helper --ignored --nocapture\n");
    let mut stage = 0u8;
    let (status, transcript) = drive_pty_until_exit(
        &mut master,
        &mut child,
        Duration::from_secs(10),
        |out, m| {
            if stage == 0 && out.contains("PROMPT> ") {
                m.write_all(run_line.as_bytes()).expect("launch bg helper");
                m.flush().expect("flush bg helper");
                stage = 1;
            } else if stage == 1 && out.contains("READY") {
                m.write_all(&[0x1a]).expect("send Ctrl-Z");
                m.flush().expect("flush Ctrl-Z");
                stage = 2;
            } else if stage == 2 && out.contains("Stopped") {
                m.write_all(b"bg\n").expect("send bg");
                m.flush().expect("flush bg");
                stage = 3;
            } else if stage == 3 && out.contains("DONE:bg-done") {
                m.write_all(b"echo AFTER_BG_OK; exit 0\n")
                    .expect("run command after bg exit");
                m.flush().expect("flush post-bg command");
                stage = 4;
            }
        },
    );
    assert_eq!(
        stage, 4,
        "bg exit test stopped at stage {stage}; transcript:\n{transcript}"
    );
    assert!(
        status.success(),
        "interactive bash failed after bg exit with {status}; transcript:\n{transcript}"
    );
    assert!(
        transcript.contains("AFTER_BG_OK"),
        "shell lost foreground after bg job exited; transcript:\n{transcript}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "internal helper invoked under a real PTY by bg_then_fg_ctrl_c_terminates_wrapped_child_under_pty"]
fn pty_bg_fg_sleep_helper() {
    if std::env::var_os(PTY_HELPER_ENV).is_none() {
        return;
    }
    let out = execute(&s(&[
        "-n",
        "5",
        "sh",
        "-c",
        "printf 'CHILD_PID=%s\\n' \"$$\" > /dev/tty; sleep 60",
    ]));
    std::process::exit(out.exit_code);
}

#[cfg(unix)]
#[test]
fn bg_then_fg_ctrl_c_terminates_wrapped_child_under_pty() {
    use std::io::Write;
    use std::process::Command;
    use std::time::Duration;

    let current_exe = std::env::current_exe().expect("resolve current test binary");
    let exe_str = current_exe.to_str().expect("utf8 test binary path");

    let mut cmd = Command::new("bash");
    cmd.args(["--norc", "--noprofile", "-i"])
        .env(PTY_HELPER_ENV, "1")
        .env("PS1", "PROMPT> ");
    let (mut master, mut child) = spawn_in_pty(&mut cmd);

    let run_line = format!(
        "\"{exe_str}\" --exact mrun::tests::pty_bg_fg_sleep_helper --ignored --nocapture\n"
    );
    let mut stage = 0u8;
    let mut wrapped_pid: Option<i32> = None;
    let (status, transcript) = drive_pty_until_exit(
        &mut master,
        &mut child,
        Duration::from_secs(10),
        |out, m| {
            if stage == 0 && out.contains("PROMPT> ") {
                m.write_all(run_line.as_bytes()).expect("launch helper");
                m.flush().expect("flush helper");
                stage = 1;
            } else if stage == 1
                && let Some(idx) = out.find("CHILD_PID=")
                && let Some(rest) = out[idx + "CHILD_PID=".len()..].split_whitespace().next()
                && let Ok(pid) = rest.parse::<i32>()
            {
                wrapped_pid = Some(pid);
                m.write_all(&[0x1a]).expect("send Ctrl-Z");
                m.flush().expect("flush Ctrl-Z");
                stage = 2;
            } else if stage == 2 && out.contains("Stopped") {
                m.write_all(b"bg\n").expect("send bg");
                m.flush().expect("flush bg");
                stage = 3;
            } else if stage == 3 && out.contains("&\r\n") && out.matches("PROMPT> ").count() >= 3 {
                m.write_all(b"fg\n").expect("send fg");
                m.flush().expect("flush fg");
                stage = 4;
            } else if stage == 4 && out.contains("fg\r\n") {
                // Give mrun's 10ms bg-watch loop time to hand foreground back to the child.
                std::thread::sleep(Duration::from_millis(50));
                m.write_all(&[0x03]).expect("send Ctrl-C");
                m.flush().expect("flush Ctrl-C");
                stage = 5;
            } else if stage == 5 && out.matches("PROMPT> ").count() >= 4 {
                m.write_all(b"exit 0\n").expect("send exit");
                m.flush().expect("flush exit");
                stage = 6;
            }
        },
    );
    assert_eq!(
        stage, 6,
        "bg->fg->Ctrl-C test stopped at stage {stage}; transcript:\n{transcript}"
    );
    assert!(
        status.success(),
        "interactive bash failed with {status}; transcript:\n{transcript}"
    );
    let pid = wrapped_pid.expect("captured wrapped child pid");
    let ps_out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("check wrapped child process state");
    let stat = String::from_utf8_lossy(&ps_out.stdout).trim().to_string();
    assert!(
        stat.is_empty() || stat.starts_with('Z'),
        "wrapped child pid {pid} survived bg->fg->Ctrl-C (stat={stat:?})"
    );
}
