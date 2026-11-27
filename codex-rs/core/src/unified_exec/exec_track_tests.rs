use std::fs;
use std::sync::Arc;

use codex_change_history::ChangeHistoryStore;
use codex_change_history::ChangeMetadata;
use codex_change_history::ChangeScope;
use codex_change_history::ChangeSourceKind;
use codex_change_history::CoverageStatus;
use codex_change_history::FileSnapshot;
use codex_change_history::PendingFileChange;
use codex_exec_track::shell_write_claim_key;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::unified_exec::exec_track::ExecTrackAttachment;

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
                author: None,
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
                scope: ChangeScope::Workspace,
            }],
        )
        .expect("record apply_patch");

    let attachment: Arc<ExecTrackAttachment> = ExecTrackAttachment::prepare_for_shell(
        "printf 'v2' > overlap.txt",
        /*session_tmp*/ None,
        ws.path().to_path_buf(),
        None,
        store_dir.path().to_path_buf(),
        "thread-overlap".to_string(),
        "sess-overlap".to_string(),
        "turn-overlap".to_string(),
        "call-overlap".to_string(),
    );
    assert!(attachment.additional_permissions().is_some());
    attachment.settle_once(Some(0));
    attachment.settle_once(Some(0));

    let entries = store.list_entries().expect("list entries");
    assert_eq!(
        entries.len(),
        1,
        "overlapping shell write must not double-count"
    );
    assert_eq!(entries[0].source_kind, ChangeSourceKind::ApplyPatch);
}

#[tokio::test]
async fn exec_track_attachment_user_shell_prepares_unattributed_observer() {
    let ws = TempDir::new().expect("ws tempdir");
    let store_dir = TempDir::new().expect("store tempdir");

    let attachment = ExecTrackAttachment::prepare_for_user_shell(
        "printf 'user-edit' > user_file.txt",
        /*session_tmp*/ None,
        ws.path().to_path_buf(),
        store_dir.path().to_path_buf(),
        "thread-user-1".to_string(),
        "sess-user-1".to_string(),
        "turn-user-1".to_string(),
        "call-user-1".to_string(),
    );
    assert!(attachment.additional_permissions().is_some());
    attachment.settle_once(Some(0));
}
