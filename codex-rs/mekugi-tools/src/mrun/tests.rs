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
    use std::io::Read;
    use std::io::Write;
    use std::os::fd::FromRawFd;
    use std::os::fd::RawFd;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::Duration;
    use std::time::Instant;

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
    let mut master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };

    let current_exe = std::env::current_exe().expect("resolve current test binary");
    let mut cmd = Command::new(current_exe);
    cmd.args([
        "--exact",
        "mrun::tests::pty_stdin_read_helper",
        "--ignored",
        "--nocapture",
    ])
    .env(PTY_HELPER_ENV, "1");

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

    let mut child = cmd.spawn().expect("spawn PTY helper test");
    drop(slave);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut captured = Vec::new();
    let mut sent_input = false;
    let mut exit_status = None;

    while Instant::now() < deadline {
        if exit_status.is_none()
            && let Some(status) = child.try_wait().expect("poll PTY helper child")
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
                    if !sent_input && String::from_utf8_lossy(&captured).contains("READY") {
                        master
                            .write_all(b"hello-pty\n")
                            .expect("write line to PTY master");
                        master.flush().expect("flush PTY master");
                        sent_input = true;
                    }
                }
                Err(err) if err.raw_os_error() == Some(libc::EIO) => {
                    // Linux returns EIO on the PTY master once all slave descriptors close.
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

    let transcript = String::from_utf8_lossy(&captured);
    let status = match exit_status {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PTY helper timed out (sent_input={sent_input}); transcript:\n{transcript}");
        }
    };
    assert!(
        sent_input,
        "child never signaled READY on PTY; transcript:\n{transcript}"
    );
    assert!(
        status.success(),
        "PTY helper failed with {status}; transcript:\n{transcript}"
    );
}
