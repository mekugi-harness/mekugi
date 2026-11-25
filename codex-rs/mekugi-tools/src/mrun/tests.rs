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
mod pty_tests {
    use std::fs::File;
    use std::io::Read;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    use std::os::fd::RawFd;
    use std::os::unix::process::CommandExt;
    use std::process::Child;
    use std::process::Command;
    use std::process::ExitStatus;
    use std::time::Duration;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::execute;
    use super::s;

    const SCRIPT_ENV: &str = "MEKUGI_MRUN_PTY_SCRIPT";
    const CHECK_FG_ENV: &str = "MEKUGI_MRUN_PTY_CHECK_FG";

    /// Single shared helper entrypoint invoked inside isolated PTY sessions by the PTY tests below.
    #[test]
    #[ignore = "internal helper invoked under a real PTY by mrun::tests::pty_tests"]
    fn pty_mrun_helper() {
        let Ok(script) = std::env::var(SCRIPT_ENV) else {
            return;
        };
        let check_fg = std::env::var_os(CHECK_FG_ENV).is_some();
        // SAFETY: POSIX queries on process group and controlling terminal foreground group.
        let pgrp_before = unsafe { libc::getpgrp() };
        if check_fg {
            // SAFETY: POSIX query on controlling terminal foreground process group.
            assert_eq!(unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) }, pgrp_before);
        }

        let start = Instant::now();
        let out = execute(&s(&["-n", "5", "sh", "-c", &script]));
        let elapsed = start.elapsed();

        if check_fg {
            // SAFETY: POSIX query on controlling terminal foreground process group.
            assert_eq!(
                unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) },
                pgrp_before,
                "mrun must restore the caller's foreground process group on exit"
            );
            assert_eq!(out.stderr, "");
            // `mrun` uses a 2s pipe drain timeout; finishing under 2s proves descendants were killed.
            assert!(
                elapsed < Duration::from_secs(2),
                "mrun hung waiting for grandchild process group under PTY: {elapsed:?}"
            );
        }
        print!("DONE:{}", out.stdout);
        std::process::exit(out.exit_code);
    }

    fn helper_invocation() -> String {
        let exe = std::env::current_exe().expect("resolve current test binary");
        let exe = exe.to_str().expect("utf8 test binary path");
        format!(r#""{exe}" --exact mrun::tests::pty_tests::pty_mrun_helper --ignored --nocapture"#)
    }

    fn spawn_in_pty(cmd: &mut Command) -> (File, Child) {
        let mut master_fd: RawFd = -1;
        let mut slave_fd: RawFd = -1;
        // SAFETY: `openpty` initializes `master_fd` and `slave_fd` on success.
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
        let master = unsafe { File::from_raw_fd(master_fd) };
        let slave = unsafe { File::from_raw_fd(slave_fd) };
        // Mark close-on-exec so parallel tests cannot inherit these PTY descriptors;
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

        // SAFETY: Post-fork child setup invokes only async-signal-safe POSIX functions.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() == -1
                    || libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0 as libc::c_int) == -1
                {
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

    fn spawn_interactive_bash(script: &str) -> (File, Child) {
        let mut cmd = Command::new("bash");
        cmd.args(["--norc", "--noprofile", "-i"])
            .env(SCRIPT_ENV, script)
            .env("PS1", "PROMPT> ");
        spawn_in_pty(&mut cmd)
    }

    fn write_pty(master: &mut File, bytes: &[u8]) {
        master.write_all(bytes).expect("write to PTY master");
        master.flush().expect("flush PTY master");
    }

    fn drive_pty_until_exit<F>(
        master: &mut File,
        child: &mut Child,
        timeout: Duration,
        mut on_tick: F,
    ) -> (ExitStatus, String)
    where
        F: FnMut(&str, &mut File),
    {
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
            let ready = unsafe { libc::poll(&mut pollfd, 1, 20) };
            if ready > 0 && (pollfd.revents & (libc::POLLIN | libc::POLLHUP)) != 0 {
                let mut buf = [0u8; 1024];
                match master.read(&mut buf) {
                    Ok(0) if exit_status.is_some() => break,
                    Ok(n) => captured.extend_from_slice(&buf[..n]),
                    Err(err) if err.raw_os_error() == Some(libc::EIO) && exit_status.is_some() => {
                        break;
                    }
                    Err(err) if err.raw_os_error() == Some(libc::EIO) => {}
                    Err(err) => panic!("read PTY master failed: {err}"),
                }
            } else if exit_status.is_some() {
                break;
            }
            let transcript = String::from_utf8_lossy(&captured);
            on_tick(&transcript, master);
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

    #[test]
    fn reads_pty_stdin_and_retires_descendants() {
        let mut cmd = Command::new(std::env::current_exe().expect("resolve current test binary"));
        cmd.args([
            "--exact",
            "mrun::tests::pty_tests::pty_mrun_helper",
            "--ignored",
            "--nocapture",
        ])
        .env(
            SCRIPT_ENV,
            r#"sleep 60 & printf 'READY\n' > /dev/tty; IFS= read -r value; printf 'value=%s\n' "$value""#,
        )
        .env(CHECK_FG_ENV, "1");

        let (mut master, mut child) = spawn_in_pty(&mut cmd);
        let mut sent = false;
        let (status, transcript) = drive_pty_until_exit(
            &mut master,
            &mut child,
            Duration::from_secs(10),
            |out, m| {
                if !sent && out.contains("READY") {
                    write_pty(m, b"hello-pty\n");
                    sent = true;
                }
            },
        );
        assert!(
            sent,
            "child never signaled READY; transcript:\n{transcript}"
        );
        assert!(
            status.success(),
            "helper failed ({status}); transcript:\n{transcript}"
        );
        assert!(
            transcript.contains("DONE:value=hello-pty"),
            "missing read output; transcript:\n{transcript}"
        );
    }

    #[test]
    fn ctrl_c_interrupts_shell_command_list_and_pipeline_under_pty() {
        let helper = helper_invocation();

        // 1. Noninteractive shell command list: Ctrl-C must abort before running the next command.
        let mut list_cmd = Command::new("bash");
        list_cmd
            .args(["-c", &format!("{helper}; printf 'CONTINUED\\n'")])
            .env(SCRIPT_ENV, r"printf 'READY\n' > /dev/tty; sleep 60");
        let (mut master, mut child) = spawn_in_pty(&mut list_cmd);
        let mut sent = false;
        let (status, transcript) = drive_pty_until_exit(
            &mut master,
            &mut child,
            Duration::from_secs(10),
            |out, m| {
                if !sent && out.contains("READY") {
                    write_pty(m, &[0x03]);
                    sent = true;
                }
            },
        );
        assert!(
            sent && !status.success() && !transcript.contains("CONTINUED"),
            "transcript:\n{transcript}"
        );

        // 2. Interactive pipeline: Ctrl-C must interrupt downstream `tee` too (status 130, not 0).
        let (mut master, mut child) =
            spawn_interactive_bash(r"printf 'READY\n' > /dev/tty; sleep 60");
        let pipeline = format!("{helper} | tee /dev/null\n");
        let mut stage = 0u8;
        let (status, transcript) = drive_pty_until_exit(
            &mut master,
            &mut child,
            Duration::from_secs(10),
            |out, m| {
                if stage == 0 && out.contains("PROMPT> ") {
                    write_pty(m, pipeline.as_bytes());
                    stage = 1;
                } else if stage == 1 && out.contains("READY") {
                    write_pty(m, &[0x03]);
                    stage = 2;
                } else if stage == 2 && out.matches("PROMPT> ").count() >= 2 {
                    write_pty(m, b"printf 'PIPE_STATUS=%s\\n' \"$?\"; exit 0\n");
                    stage = 3;
                }
            },
        );
        assert_eq!(
            stage, 3,
            "stopped at stage {stage}; transcript:\n{transcript}"
        );
        assert!(
            status.success() && transcript.contains("PIPE_STATUS=130"),
            "transcript:\n{transcript}"
        );
    }

    #[test]
    fn ctrl_z_suspends_and_resumes_under_pty() {
        let (mut master, mut child) = spawn_interactive_bash(
            r#"printf 'READY\n' > /dev/tty; IFS= read -r value; printf 'resumed=%s\n' "$value""#,
        );
        let run_line = format!("{}\n", helper_invocation());
        let mut stage = 0u8;
        let (status, transcript) = drive_pty_until_exit(
            &mut master,
            &mut child,
            Duration::from_secs(10),
            |out, m| {
                if stage == 0 && out.contains("PROMPT> ") {
                    write_pty(m, run_line.as_bytes());
                    stage = 1;
                } else if stage == 1 && out.contains("READY") {
                    write_pty(m, &[0x1a]);
                    stage = 2;
                } else if stage == 2 && out.contains("Stopped") {
                    write_pty(m, b"fg\n");
                    stage = 3;
                } else if stage == 3 && out.contains("fg\r\n") {
                    write_pty(m, b"after-fg\n");
                    stage = 4;
                } else if stage == 4 && out.contains("DONE:resumed=after-fg") {
                    write_pty(m, b"exit 0\n");
                    stage = 5;
                }
            },
        );
        assert_eq!(
            stage, 5,
            "stopped at stage {stage}; transcript:\n{transcript}"
        );
        assert!(
            status.success(),
            "bash failed ({status}); transcript:\n{transcript}"
        );
    }

    #[test]
    fn bg_exit_preserves_shell_foreground_under_pty() {
        let dir = tempfile::tempdir().expect("create gate tempdir");
        let gate = dir.path().join("release");
        let gate_str = gate.to_str().expect("utf8 gate path");
        // Block on an explicit gate file instead of a sleep so slow runners cannot exit before Ctrl-Z.
        let script = format!(
            "printf 'READY\\n' > /dev/tty; until [ -f '{gate_str}' ]; do sleep 0.01; done; printf 'bg-done\\n'"
        );
        let (mut master, mut child) = spawn_interactive_bash(&script);
        let run_line = format!("{}\n", helper_invocation());
        let mut stage = 0u8;
        let (status, transcript) = drive_pty_until_exit(
            &mut master,
            &mut child,
            Duration::from_secs(10),
            |out, m| {
                if stage == 0 && out.contains("PROMPT> ") {
                    write_pty(m, run_line.as_bytes());
                    stage = 1;
                } else if stage == 1 && out.contains("READY") {
                    write_pty(m, &[0x1a]);
                    stage = 2;
                } else if stage == 2 && out.contains("Stopped") {
                    write_pty(m, b"bg\n");
                    stage = 3;
                } else if stage == 3
                    && out.contains("&\r\n")
                    && out.matches("PROMPT> ").count() >= 3
                {
                    std::fs::write(&gate, b"go").expect("release bg child");
                    stage = 4;
                } else if stage == 4 && out.contains("DONE:bg-done") {
                    write_pty(m, b"echo AFTER_BG_OK; exit 0\n");
                    stage = 5;
                }
            },
        );
        assert_eq!(
            stage, 5,
            "stopped at stage {stage}; transcript:\n{transcript}"
        );
        assert!(
            status.success() && transcript.contains("AFTER_BG_OK"),
            "shell lost foreground after bg exit; transcript:\n{transcript}"
        );
    }

    #[test]
    fn bg_then_fg_ctrl_c_terminates_wrapped_child_under_pty() {
        let (mut master, mut child) =
            spawn_interactive_bash(r#"printf 'CHILD_PID=%s\n' "$$" > /dev/tty; sleep 60"#);
        let run_line = format!("{}\n", helper_invocation());
        let mut stage = 0u8;
        let mut wrapped_pid: Option<libc::pid_t> = None;
        let (status, transcript) = drive_pty_until_exit(
            &mut master,
            &mut child,
            Duration::from_secs(10),
            |out, m| {
                if stage == 0 && out.contains("PROMPT> ") {
                    write_pty(m, run_line.as_bytes());
                    stage = 1;
                } else if stage == 1
                    && let Some(idx) = out.find("CHILD_PID=")
                    && let Some(raw) = out[idx + "CHILD_PID=".len()..].split_whitespace().next()
                    && let Ok(pid) = raw.parse::<libc::pid_t>()
                {
                    wrapped_pid = Some(pid);
                    write_pty(m, &[0x1a]);
                    stage = 2;
                } else if stage == 2 && out.contains("Stopped") {
                    write_pty(m, b"bg\n");
                    stage = 3;
                } else if stage == 3
                    && out.contains("&\r\n")
                    && out.matches("PROMPT> ").count() >= 3
                {
                    write_pty(m, b"fg\n");
                    stage = 4;
                } else if stage == 4
                    && let Some(pid) = wrapped_pid
                    // Observe actual kernel foreground handoff rather than sleeping.
                    // SAFETY: `tcgetpgrp` is a thread-safe POSIX query on the PTY master fd.
                    && unsafe { libc::tcgetpgrp(m.as_raw_fd()) } == pid
                {
                    write_pty(m, &[0x03]);
                    stage = 5;
                } else if stage == 5 && out.matches("PROMPT> ").count() >= 4 {
                    write_pty(m, b"exit 0\n");
                    stage = 6;
                }
            },
        );
        assert_eq!(
            stage, 6,
            "stopped at stage {stage}; transcript:\n{transcript}"
        );
        assert!(
            status.success(),
            "bash failed ({status}); transcript:\n{transcript}"
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
}
