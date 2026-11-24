use std::fs;
use std::process::Stdio;
use std::sync::Arc;

use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use codex_exec_server::ExecOutputStream;
use codex_exec_track::shell_write_claim_key;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::process::Command;

use crate::shell::ShellType;
use crate::unified_exec::exec_track::ExecTrackAttachment;

#[tokio::test]
async fn exec_track_attachment_persists_shell_write_and_mktemp_once_even_on_nonzero_exit() {
    let ws = TempDir::new().expect("ws tempdir");
    let store_dir = TempDir::new().expect("store tempdir");
    let tmp_dir = ws.path().join("tmp");
    fs::create_dir_all(&tmp_dir).expect("mkdir tmp");

    let script = format!(
        "printf 'before-fail' > kept.txt; f=$(mktemp '{t}/item.XXXXXX') && printf 'mk-data' > \"$f\"; exit 5",
        t = tmp_dir.display()
    );

    let prepared = ExecTrackAttachment::prepare_for_shell(
        &script,
        ws.path(),
        /*session_tmp*/ None,
        ws.path().to_path_buf(),
        None,
        &ShellType::Bash,
        /*tty*/ false,
        store_dir.path().to_path_buf(),
        "thread-core-1".to_string(),
        "sess-core-1".to_string(),
        "turn-core-1".to_string(),
        "call-core-1".to_string(),
    );

    let instrumented = prepared
        .instrumented_script
        .as_deref()
        .expect("bash script should be instrumented");
    assert!(prepared.attachment.additional_permissions().is_some());

    let output = Command::new("/bin/bash")
        .arg("-c")
        .arg(instrumented)
        .current_dir(ws.path())
        .stdin(Stdio::null())
        .output()
        .await
        .expect("run instrumented bash");
    assert_eq!(output.status.code(), Some(5));

    prepared
        .attachment
        .push_output(ExecOutputStream::Stdout, &output.stdout);
    // Calling settle_once multiple times (e.g. immediate exit + watcher) must only persist once.
    prepared.attachment.settle_once(output.status.code());
    prepared.attachment.settle_once(output.status.code());

    let store = ChangeHistoryStore::open_existing(store_dir.path()).expect("open store");
    let entries = store.list_entries().expect("list entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source_kind, ChangeSourceKind::ShellWrite);
    assert_eq!(entries[0].exit_code, Some(5));

    let record = store.get_record(&entries[0].id).expect("get record");
    assert_eq!(record.files.len(), 2, "files={:?}", record.files);
    assert!(record.files.iter().any(|f| f.rel_path == "kept.txt"));
}

#[tokio::test]
async fn exec_track_attachment_deduplicates_against_existing_apply_patch_claim() {
    let ws = TempDir::new().expect("ws tempdir");
    let store_dir = TempDir::new().expect("store tempdir");
    let target = ws.path().join("overlap.txt");
    fs::write(&target, b"v1").expect("write v1");

    let claim = shell_write_claim_key("thread-overlap", "call-overlap");
    let store = ChangeHistoryStore::open(store_dir.path()).expect("open store");
    store
        .record_change(
            ChangeMetadata {
                thread_id: "thread-overlap".to_string(),
                session_id: "sess-overlap".to_string(),
                turn_id: "turn-overlap".to_string(),
                call_id: "call-overlap".to_string(),
                source_kind: ChangeSourceKind::ApplyPatch,
                workspace_root: ws.path().to_path_buf(),
                created_at: 1_700_000_000,
                exit_code: Some(0),
                summary: Some("apply_patch".to_string()),
                claim_key: Some(claim),
            },
            vec![PendingFileChange {
                rel_path: "overlap.txt".to_string(),
                abs_path: target.clone(),
                moved_from: None,
                before: FileSnapshot::Present(b"v1".to_vec()),
                after: FileSnapshot::Present(b"v2".to_vec()),
                coverage: CoverageStatus::Complete,
            }],
        )
        .expect("record apply_patch");

    let script = "printf 'v2' > overlap.txt";
    let prepared = ExecTrackAttachment::prepare_for_shell(
        script,
        ws.path(),
        /*session_tmp*/ None,
        ws.path().to_path_buf(),
        None,
        &ShellType::Bash,
        /*tty*/ false,
        store_dir.path().to_path_buf(),
        "thread-overlap".to_string(),
        "sess-overlap".to_string(),
        "turn-overlap".to_string(),
        "call-overlap".to_string(),
    );
    fs::write(&target, b"v2").expect("write v2");
    let attachment: Arc<ExecTrackAttachment> = prepared.attachment;
    attachment.settle_once(Some(0));

    let entries = store.list_entries().expect("list entries");
    assert_eq!(
        entries.len(),
        1,
        "overlapping shell write must not double-count"
    );
    assert_eq!(entries[0].source_kind, ChangeSourceKind::ApplyPatch);
}
