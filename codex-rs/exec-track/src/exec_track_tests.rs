use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use codex_change_history::ChangeAuthor;
use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeScope;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::WriteObserverSession;
use crate::ipc::handle_framed_message;
use crate::ipc::serve_unix_message;
use crate::persist::ObservationState;

fn sample_metadata(thread_id: &str, call_id: &str, workspace_root: PathBuf) -> ChangeMetadata {
    ChangeMetadata {
        author: Some(ChangeAuthor::from_thread_id(thread_id)),
        thread_id: thread_id.to_string(),
        session_id: "sess-1".to_string(),
        turn_id: "turn-1".to_string(),
        call_id: call_id.to_string(),
        source_kind: if thread_id.is_empty() {
            ChangeSourceKind::UserShell
        } else {
            ChangeSourceKind::ShellWrite
        },
        workspace_root,
        created_at: 1_700_000_000,
        exit_code: Some(0),
        summary: Some("test".to_string()),
        claim_key: None,
    }
}

#[test]
fn concurrent_agents_same_worktree_different_files_stay_isolated() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let file_a = workspace.path().join("a.txt");
    let file_b = workspace.path().join("b.txt");
    fs::write(&file_a, b"a-before").expect("write a");
    fs::write(&file_b, b"b-before").expect("write b");

    let obs_a = WriteObserverSession::start("call-a".to_string(), |_| true).expect("obs a");
    let obs_b = WriteObserverSession::start("call-b".to_string(), |_| true).expect("obs b");

    // Simulate concurrent pre-write hooks from Agent A and Agent B on the same worktree.
    obs_a.state.lock().unwrap().record_pre_write(&file_a);
    obs_b.state.lock().unwrap().record_pre_write(&file_b);
    fs::write(&file_a, b"a-after").expect("write a after");
    fs::write(&file_b, b"b-after").expect("write b after");

    let rec_a = obs_a
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-a", "call-a", workspace.path().to_path_buf()),
        )
        .expect("persist a")
        .expect("record a");
    let rec_b = obs_b
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-b", "call-b", workspace.path().to_path_buf()),
        )
        .expect("persist b")
        .expect("record b");

    assert_eq!(rec_a.id.as_str(), "amber1");
    assert_eq!(rec_b.id.as_str(), "apple1");
    assert_eq!(rec_a.files.len(), 1);
    assert_eq!(rec_a.files[0].rel_path, "a.txt");
    assert_eq!(rec_a.files[0].coverage, CoverageStatus::Complete);
    assert_eq!(rec_b.files.len(), 1);
    assert_eq!(rec_b.files[0].rel_path, "b.txt");
    assert_eq!(rec_b.files[0].coverage, CoverageStatus::Complete);
}

#[test]
fn concurrent_agents_same_file_marks_partial_coverage() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let shared = workspace.path().join("shared.txt");
    fs::write(&shared, b"v0").expect("write v0");

    let obs_a = WriteObserverSession::start("call-a".to_string(), |_| true).expect("obs a");
    let obs_b = WriteObserverSession::start("call-b".to_string(), |_| true).expect("obs b");

    obs_a.state.lock().unwrap().record_pre_write(&shared);
    obs_b.state.lock().unwrap().record_pre_write(&shared);
    fs::write(&shared, b"v2").expect("write v2");

    let rec_a = obs_a
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-a", "call-a", workspace.path().to_path_buf()),
        )
        .expect("persist a")
        .expect("record a");
    let rec_b = obs_b
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-b", "call-b", workspace.path().to_path_buf()),
        )
        .expect("persist b")
        .expect("record b");

    assert!(!rec_a.files[0].coverage.is_complete());
    assert!(!rec_b.files[0].coverage.is_complete());
}

#[test]
fn captures_workspace_external_and_ephemeral_scopes_and_unattributed_user_shell() {
    let home = std::env::var_os("HOME").map(PathBuf::from).expect("HOME");
    let root = tempfile::tempdir_in(&home).expect("non-tmp root");
    let workspace = root.path().join("project");
    let external_dir = root.path().join("outside");
    let session_tmp = root.path().join("session-tmp");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&external_dir).unwrap();
    fs::create_dir_all(&session_tmp).unwrap();
    let store_dir = TempDir::new().expect("store");

    let ws_file = workspace.join("main.rs");
    let ext_file = external_dir.join("notes.txt");
    let tmp_file = session_tmp.join("scratch.log");
    fs::write(&ws_file, b"old").unwrap();

    let obs = WriteObserverSession::start("call-shell".to_string(), |_| true).expect("obs");
    {
        let mut st = obs.state.lock().unwrap();
        st.record_pre_write(&ws_file);
        st.record_pre_write(&ext_file);
        st.record_pre_write(&tmp_file);
    }
    fs::write(&ws_file, b"new").unwrap();
    fs::write(&ext_file, b"outside-data").unwrap();
    fs::write(&tmp_file, b"temp-data").unwrap();

    let rec = obs
        .finish_and_persist(
            store_dir.path(),
            &workspace,
            Some(&session_tmp),
            sample_metadata("", "call-shell", workspace.clone()),
        )
        .expect("persist")
        .expect("record");

    assert_eq!(rec.id.as_str(), "shell1");
    assert!(rec.metadata.is_unattributed());
    assert_eq!(rec.metadata.thread_id, "");
    assert_eq!(rec.files.len(), 3);

    let ws_ev = rec
        .files
        .iter()
        .find(|f| f.abs_path.ends_with("main.rs"))
        .unwrap();
    assert_eq!(ws_ev.scope, ChangeScope::Workspace);
    let ext_ev = rec
        .files
        .iter()
        .find(|f| f.abs_path.ends_with("notes.txt"))
        .unwrap();
    assert_eq!(ext_ev.scope, ChangeScope::External);
    let tmp_ev = rec
        .files
        .iter()
        .find(|f| f.abs_path.ends_with("scratch.log"))
        .unwrap();
    assert_eq!(tmp_ev.scope, ChangeScope::Ephemeral);
}

#[test]
fn ipc_protocol_handles_write_and_sip_partial_markers() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let target = workspace.path().join("hello.txt");
    fs::write(&target, b"before").unwrap();

    let state = Arc::new(Mutex::new(ObservationState::new(
        "ipc-call".to_string(),
        store_dir.path().to_path_buf(),
        Arc::new(|_| true),
    )));

    let (mut client, server) = UnixStream::pair().expect("socketpair");
    let state_bg = Arc::clone(&state);
    let handle = std::thread::spawn(move || serve_unix_message(server, &state_bg));
    let mut msg = vec![b'W'];
    msg.extend_from_slice(target.to_string_lossy().as_bytes());
    msg.push(0);
    std::io::Write::write_all(&mut client, &msg).unwrap();
    let mut ack = [0u8; 1];
    std::io::Read::read_exact(&mut client, &mut ack).unwrap();
    assert_eq!(ack, [1u8]);
    handle.join().unwrap();

    handle_framed_message(b"S/bin/rm\0", &state);
    fs::write(&target, b"after").unwrap();

    let rec = state
        .lock()
        .unwrap()
        .finalize_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("t1", "ipc-call", workspace.path().to_path_buf()),
        )
        .expect("persist")
        .expect("record");
    assert_eq!(rec.files.len(), 1);
    assert!(matches!(
        &rec.files[0].coverage,
        CoverageStatus::Partial { reason } if reason.contains("SIP-protected binary: /bin/rm")
    ));
}

#[test]
fn sandbox_read_policy_blocks_unreadable_paths() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let allowed = workspace.path().join("ok.txt");
    let secret = workspace.path().join("secret.txt");
    fs::write(&allowed, b"old").unwrap();
    fs::write(&secret, b"top-secret").unwrap();
    let secret_canon = fs::canonicalize(&secret).unwrap();

    let obs = WriteObserverSession::start("call-pol".to_string(), move |p| p != secret_canon)
        .expect("obs");
    {
        let mut st = obs.state.lock().unwrap();
        st.record_pre_write(&allowed);
        st.record_pre_write(&secret);
    }
    fs::write(&allowed, b"new").unwrap();
    fs::write(&secret, b"mutated").unwrap();

    let rec = obs
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("t1", "call-pol", workspace.path().to_path_buf()),
        )
        .expect("persist")
        .expect("record");
    assert_eq!(rec.files.len(), 1);
    assert_eq!(rec.files[0].rel_path, "ok.txt");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_seccomp_unotify_captures_dynamic_shell_and_rename_writes_without_git() {
    use std::process::Command;

    let workspace = TempDir::new().expect("non-git workspace");
    let store_dir = TempDir::new().expect("store");
    let existing = workspace.path().join("target.txt");
    fs::write(&existing, b"before\n").unwrap();

    let obs = WriteObserverSession::start("seccomp-e2e".to_string(), |_| true).expect("obs");
    let sock_path = obs.dir_path().join("observe.sock");

    // Fork a child that installs the seccomp-unotify filter directly and executes sh -c
    // with dynamic variable paths and atomic rename (mv), without needing an external binary.
    let script = format!(
        "f='{}'; printf 'after\n' > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; printf 'new\n' > '{}'",
        existing.display(),
        workspace.path().join("created.txt").display(),
    );
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        crate::linux::install_filter_and_handoff(&sock_path);
        let status = Command::new("/bin/sh").args(["-c", &script]).status();
        let code = status.ok().and_then(|s| s.code()).unwrap_or(1);
        unsafe { libc::_exit(code) };
    }
    let mut status: libc::c_int = 0;
    unsafe { libc::waitpid(pid, &mut status, 0) };
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);

    let rec = obs
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata(
                "thread-linux",
                "seccomp-e2e",
                workspace.path().to_path_buf(),
            ),
        )
        .expect("persist")
        .expect("record");

    let store = ChangeHistoryStore::open_existing(store_dir.path()).unwrap();
    let target_ev = rec
        .files
        .iter()
        .find(|f| f.rel_path == "target.txt")
        .expect("target.txt");
    assert_eq!(
        store.read_file_state(&rec.id, &target_ev.before).unwrap(),
        FileSnapshot::Present(b"before\n".to_vec())
    );
    assert_eq!(
        store.read_file_state(&rec.id, &target_ev.after).unwrap(),
        FileSnapshot::Present(b"after\n".to_vec())
    );
    let created_ev = rec
        .files
        .iter()
        .find(|f| f.rel_path == "created.txt")
        .expect("created.txt");
    assert_eq!(
        store.read_file_state(&rec.id, &created_ev.before).unwrap(),
        FileSnapshot::Missing
    );
    assert_eq!(
        store.read_file_state(&rec.id, &created_ev.after).unwrap(),
        FileSnapshot::Present(b"new\n".to_vec())
    );
}

#[test]
fn concurrent_create_then_overwrite_marks_partial_coverage() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let new_file = workspace.path().join("report.txt");

    let obs_a = WriteObserverSession::start("call-create-a".to_string(), |_| true).unwrap();
    let obs_b = WriteObserverSession::start("call-overwrite-b".to_string(), |_| true).unwrap();

    obs_a.state.lock().unwrap().record_pre_write(&new_file);
    fs::write(&new_file, b"from-a").unwrap();
    obs_b.state.lock().unwrap().record_pre_write(&new_file);
    fs::write(&new_file, b"from-b").unwrap();

    let rec_a = obs_a
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-a", "call-create-a", workspace.path().to_path_buf()),
        )
        .unwrap()
        .unwrap();
    let rec_b = obs_b
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata(
                "thread-b",
                "call-overwrite-b",
                workspace.path().to_path_buf(),
            ),
        )
        .unwrap()
        .unwrap();

    assert!(!rec_a.files[0].coverage.is_complete());
    assert!(!rec_b.files[0].coverage.is_complete());
}

#[test]
fn mixed_authored_and_vcs_diagnostic_writes_persist_separate_records() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let authored = workspace.path().join("a.rs");
    let restored = workspace.path().join("b.rs");
    fs::write(&authored, b"old-a").unwrap();
    fs::write(&restored, b"dirty-b").unwrap();

    let obs = WriteObserverSession::start("call-mixed".to_string(), |_| true).unwrap();
    {
        let mut st = obs.state.lock().unwrap();
        st.record_pre_write_with_provenance(&authored, false);
        st.record_pre_write_with_provenance(&restored, true);
    }
    fs::write(&authored, b"fmt-a").unwrap();
    fs::write(&restored, b"clean-b").unwrap();

    let rec = obs
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-mix", "call-mixed", workspace.path().to_path_buf()),
        )
        .unwrap()
        .unwrap();

    assert_eq!(rec.metadata.source_kind, ChangeSourceKind::ShellWrite);
    assert_eq!(rec.files.len(), 1);
    assert_eq!(rec.files[0].rel_path, "a.rs");

    let store = ChangeHistoryStore::open_existing(store_dir.path()).unwrap();
    let entries = store.list_entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .any(|e| e.source_kind == ChangeSourceKind::VcsDiagnostic
                && e.file_paths == vec!["b.rs"])
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_background_descendant_write_succeeds_after_settle() {
    use std::process::Command;

    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let bg_out = workspace.path().join("bg_after_settle.txt");
    let bg_tmp = workspace.path().join("bg_after_settle.txt.tmp");
    let bg_go = workspace.path().join("bg_go");
    let obs = WriteObserverSession::start("seccomp-bg".to_string(), |_| true).unwrap();
    let sock_path = obs.dir_path().join("observe.sock");

    let script = format!(
        "(while [ ! -f '{}' ]; do sleep 0.02; done; printf 'ok' > '{}' && mv '{}' '{}') >/dev/null 2>&1 &",
        bg_go.display(),
        bg_tmp.display(),
        bg_tmp.display(),
        bg_out.display()
    );
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        crate::linux::install_filter_and_handoff(&sock_path);
        let status = Command::new("/bin/sh").args(["-c", &script]).status();
        let code = status.ok().and_then(|s| s.code()).unwrap_or(1);
        unsafe { libc::_exit(code) };
    }
    let mut status: libc::c_int = 0;
    unsafe { libc::waitpid(pid, &mut status, 0) };
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);

    let _ = obs.finish_and_persist(
        store_dir.path(),
        workspace.path(),
        None,
        sample_metadata("thread-bg", "seccomp-bg", workspace.path().to_path_buf()),
    );
    fs::write(&bg_go, b"go").expect("signal background writer after settle");

    for _ in 0..30 {
        if bg_out.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        fs::read(&bg_out).expect("bg output must succeed without ENOSYS"),
        b"ok"
    );
}

#[test]
fn stop_before_finish_and_persist_preserves_pre_write_baselines() {
    let workspace = TempDir::new().expect("workspace");
    let store_dir = TempDir::new().expect("store");
    let target = workspace.path().join("timed_out.txt");
    fs::write(&target, b"before\n").unwrap();

    let obs = WriteObserverSession::start("timeout-call".to_string(), |_| true).unwrap();
    obs.state.lock().unwrap().record_pre_write(&target);
    fs::write(&target, b"after-before-timeout\n").unwrap();

    // Mimics terminate_confirmed stopping observation before process_manager persists.
    obs.stop();
    let record = obs
        .finish_and_persist(
            store_dir.path(),
            workspace.path(),
            None,
            sample_metadata("thread-t", "timeout-call", workspace.path().to_path_buf()),
        )
        .unwrap()
        .expect("pre-timeout write must still persist after stop()");
    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].rel_path, "timed_out.txt");
}
