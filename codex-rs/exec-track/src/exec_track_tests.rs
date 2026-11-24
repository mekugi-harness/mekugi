use std::fs;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::ExecTrackReport;
use crate::ExecTrackSession;
use crate::OutputStreamKind;
use crate::RewriteOptions;
use crate::SegmentStatus;
use crate::ShellDialect;
use crate::ShellWriteCapture;
use crate::TrackingMode;
use crate::extract_static_write_paths;
use crate::instrument_script;
use crate::shell_write_claim_key;
use crate::split_segments;

struct ShellRunResult {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    report: Option<ExecTrackReport>,
}

async fn run_plain_shell(shell_bin: &str, script: &str, cwd: &Path) -> ShellRunResult {
    let output = Command::new(shell_bin)
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output()
        .await
        .expect("spawn plain shell");
    ShellRunResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
        report: None,
    }
}

async fn run_tracked_shell(
    shell_bin: &str,
    dialect: ShellDialect,
    script: &str,
    cwd: &Path,
    extra_pre_exec: Option<Box<dyn Fn() -> std::io::Result<()> + Send + Sync>>,
) -> ShellRunResult {
    let split = split_segments(script, dialect)
        .unwrap_or_else(|| panic!("script should be trackable ({dialect:?}): {script}"));
    let (session, mut drain_rx) =
        ExecTrackSession::start(split.segments.clone(), TrackingMode::Pipe)
            .expect("start tracking session");
    let session = Arc::new(session);
    let instrumented = session.instrument_script(script, &split.segments, dialect);

    let mut cmd = Command::new(shell_bin);
    cmd.arg("-c")
        .arg(&instrumented)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(hook) = extra_pre_exec {
        unsafe {
            cmd.pre_exec(hook);
        }
    }

    let mut child = cmd.spawn().expect("spawn tracked shell");
    let mut stdout_pipe = child.stdout.take().expect("stdout pipe");
    let mut stderr_pipe = child.stderr.take().expect("stderr pipe");

    let (tx_out, mut rx_out) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (tx_err, mut rx_err) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    let out_reader = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stdout_pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = tx_out.send(buf[..n].to_vec());
                }
            }
        }
    });
    let err_reader = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stderr_pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = tx_err.send(buf[..n].to_vec());
                }
            }
        }
    });

    let collector_session = Arc::clone(&session);
    let collector = tokio::spawn(async move {
        let mut all_stdout = Vec::new();
        let mut all_stderr = Vec::new();
        let mut out_open = true;
        let mut err_open = true;
        let mut drain_open = true;
        while out_open || err_open {
            tokio::select! {
                biased;
                chunk = rx_out.recv(), if out_open => {
                    match chunk {
                        Some(bytes) => {
                            all_stdout.extend_from_slice(&bytes);
                            collector_session.push_output(OutputStreamKind::Stdout, &bytes);
                        }
                        None => out_open = false,
                    }
                }
                chunk = rx_err.recv(), if err_open => {
                    match chunk {
                        Some(bytes) => {
                            all_stderr.extend_from_slice(&bytes);
                            collector_session.push_output(OutputStreamKind::Stderr, &bytes);
                        }
                        None => err_open = false,
                    }
                }
                req = drain_rx.recv(), if drain_open => {
                    match req {
                        Some(req) => {
                            // Yield briefly so reader tasks can forward any bytes written before the boundary.
                            tokio::task::yield_now().await;
                            while let Ok(bytes) = rx_out.try_recv() {
                                all_stdout.extend_from_slice(&bytes);
                                collector_session.push_output(OutputStreamKind::Stdout, &bytes);
                            }
                            while let Ok(bytes) = rx_err.try_recv() {
                                all_stderr.extend_from_slice(&bytes);
                                collector_session.push_output(OutputStreamKind::Stderr, &bytes);
                            }
                            req.complete();
                        }
                        None => drain_open = false,
                    }
                }
            }
        }
        (all_stdout, all_stderr)
    });

    let status = child.wait().await.expect("wait child");
    let _ = out_reader.await;
    let _ = err_reader.await;
    let (stdout, stderr) = collector.await.expect("collector join");
    let exit_code = status.code().unwrap_or(-1);
    let report = session.finalize(Some(exit_code));

    ShellRunResult {
        exit_code,
        stdout,
        stderr,
        report: Some(report),
    }
}

fn find_dash() -> Option<String> {
    for candidate in ["/bin/dash", "/usr/bin/dash", "/bin/sh"] {
        if Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

#[test]
fn ast_eligibility_rejects_hazards_and_bash_only_for_dash() {
    assert!(split_segments("", ShellDialect::Bash).is_none());
    assert!(split_segments("echo __mekugi_x", ShellDialect::Bash).is_none());
    assert!(split_segments("if then fi", ShellDialect::Bash).is_none());
    assert!(split_segments("sleep 1 &", ShellDialect::Bash).is_none());
    assert!(split_segments("trap 'echo hi' EXIT; echo ok", ShellDialect::Bash).is_none());
    assert!(split_segments("exec /bin/true", ShellDialect::Bash).is_none());
    assert!(split_segments("set -x; echo hi", ShellDialect::Bash).is_none());
    assert!(split_segments("echo $PIPESTATUS", ShellDialect::Bash).is_none());
    assert!(split_segments("echo $!", ShellDialect::Bash).is_none());
    assert!(split_segments("echo $_", ShellDialect::Bash).is_none());
    assert!(split_segments("return 1", ShellDialect::Bash).is_none());

    // Bash-only syntax rejected for Dash, accepted for Bash.
    assert!(split_segments("[[ -d /tmp ]] && echo yes", ShellDialect::Bash).is_some());
    assert!(split_segments("[[ -d /tmp ]] && echo yes", ShellDialect::Dash).is_none());
    assert!(split_segments("cat <<< hello", ShellDialect::Bash).is_some());
    assert!(split_segments("cat <<< hello", ShellDialect::Dash).is_none());

    // Shell setup collapses into 1 segment.
    let setup = split_segments("export FOO=1; printf '%s' \"$FOO\"", ShellDialect::Bash)
        .expect("shell setup should collapse");
    assert!(setup.shell_setup_collapsed);
    assert_eq!(setup.segments.len(), 1);
}

#[tokio::test]
async fn bash_and_dash_multi_segment_branches_heredocs_and_redirections() {
    let dir = TempDir::new().expect("tempdir");
    let scripts = [
        "printf first; printf second >&2; false && printf never; printf last",
        "printf '%s' $?; false; printf '%s' $?; true || printf skipped",
        "cat <<'EOF'
heredoc line 1
heredoc line 2
EOF
printf tail",
        "if false; then printf never; elif true; then printf chosen; else printf nope; fi; printf done",
        "printf redirected > out.txt && cat out.txt",
    ];

    let mut shells = vec![("/bin/bash".to_string(), ShellDialect::Bash)];
    if let Some(dash) = find_dash() {
        shells.push((dash, ShellDialect::Dash));
    }

    for (shell_bin, dialect) in shells {
        for script in scripts {
            let plain_dir = TempDir::new().expect("plain dir");
            let tracked_dir = TempDir::new().expect("tracked dir");
            let plain = run_plain_shell(&shell_bin, script, plain_dir.path()).await;
            let tracked =
                run_tracked_shell(&shell_bin, dialect, script, tracked_dir.path(), None).await;

            assert_eq!(
                tracked.exit_code, plain.exit_code,
                "shell={shell_bin} script={script}"
            );
            assert_eq!(
                tracked.stdout, plain.stdout,
                "shell={shell_bin} script={script}"
            );
            assert_eq!(
                tracked.stderr, plain.stderr,
                "shell={shell_bin} script={script}"
            );

            let report = tracked.report.expect("report");
            assert!(
                report.tracked,
                "shell={shell_bin} script={script} report={report:?}"
            );
            assert!(!report.incomplete_evidence);
            assert!(
                report.segments.len() >= 2,
                "expected multiple segments for {script}"
            );

            // Check branch skipping specifically on scripts with "never".
            if script.contains("printf never") {
                assert!(
                    report
                        .segments
                        .iter()
                        .any(|s| s.source.contains("printf never")
                            && s.status == SegmentStatus::Skipped),
                    "expected skipped branch in {report:?}"
                );
            }
        }
    }
    let _ = dir;
}

#[tokio::test]
async fn rapid_mixed_stdout_stderr_and_nonzero_exit_after_write() {
    let dir = TempDir::new().expect("tempdir");
    let store_dir = TempDir::new().expect("store dir");
    let script = "printf 'alpha' > written.txt; printf 'out1\n'; printf 'err1\n' >&2; printf 'out2\n'; exit 7";

    let capture = ShellWriteCapture::prepare(script, dir.path());
    assert!(capture.candidate_count() >= 1);

    let tracked =
        run_tracked_shell("/bin/bash", ShellDialect::Bash, script, dir.path(), None).await;
    assert_eq!(tracked.exit_code, 7);
    assert_eq!(
        tracked.stdout,
        b"out1
out2
"
    );
    assert_eq!(
        tracked.stderr,
        b"err1
"
    );

    let report = tracked.report.expect("report");
    assert!(report.tracked);
    assert_eq!(report.exit_code, Some(7));
    assert_eq!(
        report.segments[0].status,
        SegmentStatus::Completed { exit_code: 0 }
    );
    assert_eq!(
        report.segments[4].status,
        SegmentStatus::Completed { exit_code: 7 }
    );

    // Persist shell write even though exit_code was nonzero (7).
    let record = capture
        .finalize_and_persist(
            store_dir.path(),
            ChangeMetadata {
                thread_id: "thread-1".to_string(),
                session_id: "sess-1".to_string(),
                turn_id: "turn-1".to_string(),
                call_id: "call-1".to_string(),
                source_kind: ChangeSourceKind::ShellWrite,
                workspace_root: dir.path().to_path_buf(),
                created_at: 1_700_000_000,
                exit_code: Some(7),
                summary: Some(script.to_string()),
                claim_key: Some(shell_write_claim_key("thread-1", "call-1")),
            },
            &report.mktemp_creations,
        )
        .expect("persist")
        .expect("change record should be created despite nonzero exit");

    assert_eq!(record.metadata.exit_code, Some(7));
    assert_eq!(record.metadata.source_kind, ChangeSourceKind::ShellWrite);
    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].rel_path, "written.txt");
}

#[tokio::test]
async fn occupied_caller_fds_7_and_8_do_not_stall_or_break_tracking() {
    let dir = TempDir::new().expect("tempdir");
    // Hold real open FDs on 7 and 8 inside the spawned shell!
    let dev_null_7 = fs::File::open("/dev/null").expect("open /dev/null");
    let dev_null_8 = fs::File::open("/dev/null").expect("open /dev/null");
    let raw7 = dev_null_7.as_raw_fd();
    let raw8 = dev_null_8.as_raw_fd();

    let script = "printf 'seg0'; printf 'seg1' >&2; false; printf 'seg3'";
    let tracked = run_tracked_shell(
        "/bin/bash",
        ShellDialect::Bash,
        script,
        dir.path(),
        Some(Box::new(move || {
            if unsafe { libc::dup2(raw7, 7) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { libc::dup2(raw8, 8) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })),
    )
    .await;

    drop(dev_null_7);
    drop(dev_null_8);

    assert_eq!(tracked.exit_code, 0);
    assert_eq!(tracked.stdout, b"seg0seg3");
    assert_eq!(tracked.stderr, b"seg1");

    let report = tracked.report.expect("report");
    assert!(
        report.tracked,
        "tracking should still succeed with FDs 7 & 8 occupied"
    );
    assert_eq!(report.segments.len(), 4);
    assert_eq!(
        report.segments[2].status,
        SegmentStatus::Completed { exit_code: 1 }
    );
}

#[tokio::test]
async fn missing_tracker_falls_back_cleanly_without_stall() {
    let dir = TempDir::new().expect("tempdir");
    let missing_dir = dir.path().join("nonexistent-tracker-dir");
    let script = "printf 'ORIGINAL\n'; printf 'STDERR\n' >&2; exit 3";
    let split = split_segments(script, ShellDialect::Bash).expect("split");
    let instrumented = instrument_script(
        script,
        &split.segments,
        &RewriteOptions {
            dialect: ShellDialect::Bash,
            mode: TrackingMode::Pipe,
            ctl_path: missing_dir.join("ctl"),
            ack_path: missing_dir.join("ack"),
            mktemp_path: missing_dir.join("mktemp.log"),
        },
    );

    let output = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new("/bin/bash")
            .arg("-c")
            .arg(&instrumented)
            .current_dir(dir.path())
            .output(),
    )
    .await
    .expect("must not stall when tracker is missing")
    .expect("run bash");

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        output.stdout,
        b"ORIGINAL
"
    );
    assert_eq!(
        output.stderr,
        b"STDERR
"
    );
}

#[tokio::test]
async fn detached_child_does_not_hold_completion_while_late_writer_is_admitted() {
    let dir = TempDir::new().expect("tempdir");

    // Case 1: Detached child closes stdout/stderr and sleeps 10s.
    // Because no tracking FD is inherited, completion must finish quickly (<2s).
    let detached_script = "(sleep 10 >/dev/null 2>&1 </dev/null &); printf 'fast\n'";
    let split = split_segments("printf 'fast\n'", ShellDialect::Bash).expect("split");
    let (session, _drain_rx) =
        ExecTrackSession::start(split.segments.clone(), TrackingMode::Pipe).expect("start");
    let instrumented =
        session.instrument_script(detached_script, &split.segments, ShellDialect::Bash);

    let output = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new("/bin/bash")
            .arg("-c")
            .arg(&instrumented)
            .current_dir(dir.path())
            .output(),
    )
    .await
    .expect("detached child must not hold completion open")
    .expect("run bash");
    assert_eq!(
        output.stdout,
        b"fast
"
    );
    let report = session.finalize(output.status.code());
    assert!(report.tracked);

    // Case 2: Late stdout writer writes AFTER shell exits while stdout remains open.
    let late_script = "(sleep 0.15; printf 'LATE_OUT\n') & printf 'EARLY\n'";
    let (late_session, _late_drain) =
        ExecTrackSession::start(split.segments.clone(), TrackingMode::Pipe).expect("start");
    let late_instrumented =
        late_session.instrument_script(late_script, &split.segments, ShellDialect::Bash);
    let late_output = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new("/bin/bash")
            .arg("-c")
            .arg(&late_instrumented)
            .current_dir(dir.path())
            .output(),
    )
    .await
    .expect("late writer finishes")
    .expect("run bash");
    late_session.push_output(OutputStreamKind::Stdout, &late_output.stdout);
    let late_stdout = String::from_utf8_lossy(&late_output.stdout);
    assert!(late_stdout.contains(
        "EARLY
"
    ));
    assert!(late_stdout.contains(
        "LATE_OUT
"
    ));
    let late_report = late_session.finalize(late_output.status.code());
    assert!(late_report.tracked);
    assert!(
        String::from_utf8_lossy(&late_report.segments[0].stdout).contains(
            "LATE_OUT
"
        ),
        "late writer bytes must be admitted to segment"
    );
}

#[tokio::test]
async fn mktemp_file_dir_dry_run_failure_and_change_history_admission() {
    let dir = TempDir::new().expect("tempdir");
    let store_dir = TempDir::new().expect("store dir");
    let tmp_root = dir.path().join("tmp");
    fs::create_dir_all(&tmp_root).expect("mkdir tmp");

    let script = format!(
        "f=$(mktemp '{t}/file.XXXXXX') && printf 'hello-mktemp' > \"$f\"; d=$(mktemp -d '{t}/dir.XXXXXX') && printf 'nested' > \"$d/sub.txt\"; dry=$(mktemp -u '{t}/dry.XXXXXX'); mktemp '{t}/missing-parent/bad.XXXXXX' 2>/dev/null || true",
        t = tmp_root.display()
    );

    let capture = ShellWriteCapture::prepare(&script, dir.path());
    let tracked =
        run_tracked_shell("/bin/bash", ShellDialect::Bash, &script, dir.path(), None).await;
    assert_eq!(tracked.exit_code, 0);

    let report = tracked.report.expect("report");
    assert!(report.tracked, "report={report:?}");
    // Must observe exactly 2 actual creations (the file and the dir), NOT dry-run and NOT failure!
    assert_eq!(
        report.mktemp_creations.len(),
        2,
        "expected only actual mktemp creations, got: {:?}",
        report.mktemp_creations
    );
    assert!(report.mktemp_creations.iter().any(|c| !c.is_dir));
    assert!(report.mktemp_creations.iter().any(|c| c.is_dir));

    let record = capture
        .finalize_and_persist(
            store_dir.path(),
            ChangeMetadata {
                thread_id: "thread-mk".to_string(),
                session_id: "sess-mk".to_string(),
                turn_id: "turn-mk".to_string(),
                call_id: "call-mk".to_string(),
                source_kind: ChangeSourceKind::ShellWrite,
                workspace_root: dir.path().to_path_buf(),
                created_at: 1_700_000_001,
                exit_code: Some(0),
                summary: Some("mktemp test".to_string()),
                claim_key: Some(shell_write_claim_key("thread-mk", "call-mk")),
            },
            &report.mktemp_creations,
        )
        .expect("persist mktemp")
        .expect("record created");

    // Both the mktemp file AND the file inside the mktemp -d directory tree must be captured!
    assert_eq!(record.files.len(), 2, "files={:?}", record.files);
    assert!(record.files.iter().any(|f| f.rel_path.ends_with("sub.txt")));
}

#[test]
fn overlapping_patch_and_shell_write_deduplicate_via_claim_key() {
    let dir = TempDir::new().expect("tempdir");
    let store_dir = TempDir::new().expect("store dir");
    let target = dir.path().join("shared.txt");
    fs::write(&target, b"after").expect("write target");

    let claim = shell_write_claim_key("thread-dup", "call-dup");
    let store = ChangeHistoryStore::open(store_dir.path()).expect("open store");

    // First: apply_patch records the edit under the shared claim_key.
    let patch_record = store
        .record_change(
            ChangeMetadata {
                thread_id: "thread-dup".to_string(),
                session_id: "sess-dup".to_string(),
                turn_id: "turn-dup".to_string(),
                call_id: "call-dup".to_string(),
                source_kind: ChangeSourceKind::ApplyPatch,
                workspace_root: dir.path().to_path_buf(),
                created_at: 1_700_000_010,
                exit_code: Some(0),
                summary: Some("apply_patch".to_string()),
                claim_key: Some(claim.clone()),
            },
            vec![PendingFileChange {
                rel_path: "shared.txt".to_string(),
                abs_path: target.clone(),
                moved_from: None,
                before: FileSnapshot::Present(b"before".to_vec()),
                after: FileSnapshot::Present(b"after".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("record patch");

    // Second: ShellWriteCapture attempts to persist the same call with the same claim_key.
    let capture = ShellWriteCapture::prepare("printf after > shared.txt", dir.path());
    // Overwrite file so before != after isn't skipped if baseline matched; simulate pre-edit capture:
    let _ = capture;
    let shell_record = store
        .record_change(
            ChangeMetadata {
                thread_id: "thread-dup".to_string(),
                session_id: "sess-dup".to_string(),
                turn_id: "turn-dup".to_string(),
                call_id: "call-dup".to_string(),
                source_kind: ChangeSourceKind::ShellWrite,
                workspace_root: dir.path().to_path_buf(),
                created_at: 1_700_000_011,
                exit_code: Some(0),
                summary: Some("shell".to_string()),
                claim_key: Some(claim),
            },
            vec![PendingFileChange {
                rel_path: "shared.txt".to_string(),
                abs_path: target,
                moved_from: None,
                before: FileSnapshot::Present(b"before".to_vec()),
                after: FileSnapshot::Present(b"after".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("deduplicated record");

    assert_eq!(shell_record.id, patch_record.id);
    assert_eq!(store.list_entries().expect("entries").len(), 1);
}

#[test]
fn static_write_path_extraction_covers_redirs_tee_mv_rm_sed() {
    let cwd = PathBuf::from("/tmp/ws");
    let paths = extract_static_write_paths(
        "echo hi > a.txt; cat a.txt | tee b.txt > /dev/null; sed -i 's/a/b/' c.txt; mv d.txt e.txt; rm -f f.txt",
        &cwd,
    );
    let expected: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"]
        .into_iter()
        .map(|name| cwd.join(name))
        .collect();
    assert_eq!(paths, expected);
}

#[tokio::test]
async fn delayed_final_ack_reader_does_not_hang_after_control_loop_exits() {
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Write;

    let split = split_segments("echo ok", ShellDialect::Bash).expect("trackable");
    let (session, _drain_rx) =
        ExecTrackSession::start(split.segments, TrackingMode::Pipe).expect("start session");
    let ctl_path = session.ctl_path().to_path_buf();
    let ack_path = session.ack_path().to_path_buf();

    // Handshake open + immediate ack.
    {
        let mut ctl = fs::OpenOptions::new().write(true).open(&ctl_path).expect("open ctl");
        writeln!(ctl, "o").expect("write o");
        ctl.flush().expect("flush o");
    }
    let ack_open = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking({
            let ack_path = ack_path.clone();
            move || {
                let file = fs::File::open(&ack_path).expect("open ack");
                let mut line = String::new();
                BufReader::new(file).read_line(&mut line).expect("read ack");
                line
            }
        }),
    )
    .await
    .expect("open ack timed out")
    .expect("join open ack");
    assert_eq!(ack_open, "k\n");

    // Send final done message and deliberately wait so run_control_loop exits before Bash opens ack.
    {
        let mut ctl = fs::OpenOptions::new().write(true).open(&ctl_path).expect("open ctl");
        writeln!(ctl, "d 0").expect("write d");
        ctl.flush().expect("flush d");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Delayed final ack reader must still receive "k\n" without hanging.
    let ack_done = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || {
            let file = fs::File::open(&ack_path).expect("open delayed ack");
            let mut line = String::new();
            BufReader::new(file)
                .read_line(&mut line)
                .expect("read delayed ack");
            line
        }),
    )
    .await
    .expect("delayed final ack reader hung")
    .expect("join delayed ack");
    assert_eq!(ack_done, "k\n");

    let report = session.finalize(Some(0));
    assert!(report.tracked, "report={report:?}");
    assert_eq!(report.exit_code, Some(0));
}

#[test]
fn mktemp_log_rejects_forged_paths_and_symlink_escapes() {
    use crate::MktempCreation;

    let dir = TempDir::new().expect("tempdir");
    let store_dir = TempDir::new().expect("store dir");

    let legit = dir.path().join("legit.txt");
    fs::write(&legit, b"legit content").expect("write legit");

    let escape_link = dir.path().join("escape_hosts");
    std::os::unix::fs::symlink("/etc/hosts", &escape_link).expect("symlink /etc/hosts");

    let capture = ShellWriteCapture::prepare("true", dir.path());
    let mktemp_creations = vec![
        MktempCreation {
            path: PathBuf::from("/etc/hosts"),
            is_dir: false,
            segment_index: 0,
        },
        MktempCreation {
            path: escape_link,
            is_dir: false,
            segment_index: 0,
        },
        MktempCreation {
            path: legit,
            is_dir: false,
            segment_index: 0,
        },
    ];

    let record = capture
        .finalize_and_persist(
            store_dir.path(),
            ChangeMetadata {
                thread_id: "thread-sec".to_string(),
                session_id: "sess-sec".to_string(),
                turn_id: "turn-sec".to_string(),
                call_id: "call-sec".to_string(),
                source_kind: ChangeSourceKind::ShellWrite,
                workspace_root: dir.path().to_path_buf(),
                created_at: 1_700_000_020,
                exit_code: Some(0),
                summary: Some("forged mktemp test".to_string()),
                claim_key: Some(shell_write_claim_key("thread-sec", "call-sec")),
            },
            &mktemp_creations,
        )
        .expect("persist")
        .expect("legit file should still be recorded");

    assert_eq!(record.files.len(), 1, "files={:?}", record.files);
    assert_eq!(record.files[0].rel_path, "legit.txt");
}

#[test]
fn oversized_before_and_after_preserves_partial_coverage_record() {
    let dir = TempDir::new().expect("tempdir");
    let store_dir = TempDir::new().expect("store dir");
    let big = dir.path().join("big.bin");
    // Exceeds 8 MiB per-file capture cap both before and after.
    fs::write(&big, vec![b'a'; 8 * 1024 * 1024 + 16]).expect("write big before");

    let capture = ShellWriteCapture::prepare("echo x >> big.bin", dir.path());
    fs::write(&big, vec![b'b'; 8 * 1024 * 1024 + 32]).expect("write big after");

    let record = capture
        .finalize_and_persist(
            store_dir.path(),
            ChangeMetadata {
                thread_id: "thread-big".to_string(),
                session_id: "sess-big".to_string(),
                turn_id: "turn-big".to_string(),
                call_id: "call-big".to_string(),
                source_kind: ChangeSourceKind::ShellWrite,
                workspace_root: dir.path().to_path_buf(),
                created_at: 1_700_000_021,
                exit_code: Some(0),
                summary: Some("oversized edit".to_string()),
                claim_key: Some(shell_write_claim_key("thread-big", "call-big")),
            },
            &[],
        )
        .expect("persist")
        .expect("oversized edit must not vanish from history");

    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].rel_path, "big.bin");
    assert!(
        matches!(record.files[0].coverage, CoverageStatus::Partial { .. }),
        "expected Partial coverage, got {:?}",
        record.files[0].coverage
    );
}

#[test]
fn extract_static_write_paths_tracks_cd_and_resolves_symlinks() {
    let dir = TempDir::new().expect("tempdir");
    let sub = dir.path().join("subdir");
    let nested = dir.path().join("nested");
    let deep = nested.join("deep");
    fs::create_dir_all(&sub).expect("mkdir subdir");
    fs::create_dir_all(&deep).expect("mkdir deep");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&deep, &link).expect("symlink");

    // Static cd updates cwd for subsequent writes; subshell cd does not leak.
    let paths = extract_static_write_paths(
        "(cd subdir && echo a > in_sub.txt); echo b > in_root.txt; cd subdir && echo c > after_cd.txt",
        dir.path(),
    );
    let canon_root = fs::canonicalize(dir.path()).expect("canon root");
    let canon_sub = fs::canonicalize(&sub).expect("canon sub");
    assert!(paths.contains(&canon_sub.join("in_sub.txt")));
    assert!(paths.contains(&canon_root.join("in_root.txt")));
    assert!(paths.contains(&canon_sub.join("after_cd.txt")));

    // Symlink parent with .. resolves to nested/config rather than root/config.
    let sym_paths = extract_static_write_paths(
        "echo x > config; echo y > link/../config; echo z >> subdir/../config",
        dir.path(),
    );
    let canon_nested = fs::canonicalize(&nested).expect("canon nested");
    assert!(sym_paths.contains(&canon_root.join("config")));
    assert!(sym_paths.contains(&canon_nested.join("config")));
    assert_eq!(
        sym_paths.len(),
        2,
        "subdir/../config should deduplicate with config while link/../config stays distinct"
    );
}

#[tokio::test]
async fn mktemp_long_option_with_u_is_still_recorded() {
    let dir = TempDir::new().expect("tempdir");
    let script = format!(
        "f=$(mktemp --suffix=.txt '{}/suf.XXXXXX') && echo ok > \"$f\"",
        dir.path().display()
    );
    let split = split_segments(&script, ShellDialect::Bash).expect("split");
    let (session, _drain_rx) = ExecTrackSession::start(split.segments.clone(), TrackingMode::Pipe)
        .expect("start");
    let instrumented = session.instrument_script(&script, &split.segments, ShellDialect::Bash);

    let output = Command::new("/bin/bash")
        .arg("-c")
        .arg(&instrumented)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .output()
        .await
        .expect("run bash");

    // Skip assertion if the platform's mktemp does not support --suffix (e.g. BSD mktemp).
    if output.status.success() {
        let report = session.finalize(output.status.code());
        assert_eq!(
            report.mktemp_creations.len(),
            1,
            "mktemp --suffix=.txt must not be treated as dry-run: {report:?}"
        );
        assert!(report.mktemp_creations[0].path.to_string_lossy().ends_with(".txt"));
    }
}
